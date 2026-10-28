//! Provider-independent contracts used by Worker Features and trusted host composition.
//!
//! Service IDs identify a capability's semantics, not a Feature, backend, Rust type name, or
//! TypeId. Versions are exact compatibility epochs checked by the existing install planner;
//! they are not crate versions or semver ranges. Compatible implementation changes retain the
//! ID/epoch. Changes to required methods, request/result meaning, authorization guarantees, or
//! subject routing that break consumers increment the affected epoch and update providers and
//! requirements together. A different capability needs a different ID, not provider priority.
//! Rust downcasts only validate an in-process binding and are not a public compatibility ABI.
//!
//! Trusted composition selects the exact-one control provider, including the explicit combined
//! Workspace/SubWorker implementation. Acquiring a handle does not grant authority: providers
//! must revalidate current authority, Workspace ownership and direct-child constraints at each
//! operation. Observation tools bound public projections; providers reauthorize every capture.
//! These Rust contracts are not a Plugin/guest ABI; an external boundary needs an explicit adapter.

use async_trait::async_trait;
use protocol::Segment;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use session_store::LogEntry;
use ticket::{NewTicket, Result as TicketResult, TicketError, TicketRef, TicketState};

// Reuse the Host-neutral response/error vocabulary rather than redesigning transport DTOs.
use crate::worker::{WorkspaceClientError, WorkspaceResponse};

pub const WORKER_CONTROL_SERVICE_ID: &str = "worker.control";
pub const WORKER_CONTROL_SERVICE_VERSION: &str = "1";
pub const WORKER_LIFECYCLE_SERVICE_ID: &str = "worker.lifecycle";
pub const WORKER_LIFECYCLE_SERVICE_VERSION: &str = "1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerControlSubject {
    RuntimeWorker {
        runtime_id: String,
        worker_id: String,
    },
    SubWorker {
        name: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerControlInputKind {
    User,
    Notify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerControlStopKind {
    Cancel,
    Stop,
}

#[async_trait]
pub trait WorkerControlService: Send + Sync {
    fn workspace_id(&self) -> &str;
    async fn list_workers(&self) -> Result<WorkspaceResponse, WorkspaceClientError>;
    async fn send_input(
        &self,
        subject: WorkerControlSubject,
        content: String,
        kind: WorkerControlInputKind,
    ) -> Result<WorkspaceResponse, WorkspaceClientError>;
    async fn stop_worker(
        &self,
        subject: WorkerControlSubject,
        kind: WorkerControlStopKind,
        reason: Option<String>,
    ) -> Result<WorkspaceResponse, WorkspaceClientError>;
    async fn spawn_worker(
        &self,
        request: WorkerLifecycleSpawnRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError>;
    async fn restore_worker(
        &self,
        runtime_id: String,
        worker_id: String,
        request: server_api::WorkerRestoreRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        let _ = (runtime_id, worker_id, request);
        Err(WorkspaceClientError::Unavailable(
            "Runtime Worker restore is unavailable".to_string(),
        ))
    }
    fn remove_runtime_worker(
        &self,
        runtime_id: &str,
        worker_id: &str,
        reason: &str,
    ) -> Result<WorkspaceResponse, WorkspaceClientError>;
    async fn ensure_permission(
        &self,
        subject: &WorkerObservationSubjectRef,
        permission: &str,
    ) -> Result<(), WorkspaceClientError>;
}

#[async_trait]
pub trait WorkerLifecycleService: Send + Sync {
    async fn spawn(
        &self,
        request: WorkerLifecycleSpawnRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerLifecycleWorkdirAttachment {
    /// Stable Worker-local routing alias preserved into the canonical create request.
    pub alias: String,
    /// Workspace-authoritative Workdir id preserved into the canonical create request.
    pub working_directory_id: String,
    /// Optional normalized initial cwd relative to this Workdir.
    pub relative_cwd: Option<String>,
}

#[derive(Debug, Clone)]
pub struct WorkerLifecycleSpawnRequest {
    /// Observed delegation required for Orchestrator Ticket starts.
    pub orchestrator_assignment_id: Option<String>,

    pub runtime_id: String,
    /// Ordered alias-keyed selections forwarded as canonical Workdir attachments.
    pub workdir_attachments: Vec<WorkerLifecycleWorkdirAttachment>,
    pub profile: String,
    pub singleton_key: Option<String>,
    pub ticket_id: Option<String>,
    /// Required for ordinary and Ticket spawn; exact retries reuse this ID and full input.
    pub operation_id: Option<String>,
    /// Explicit requested authority. Profile selection alone grants nothing.
    pub execution_grant: Option<server_api::authorization::WorkerDelegationRequest>,
    pub display_name: String,
    pub initial_submit: Vec<Segment>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerObservationSubjectRef {
    RuntimeWorker {
        runtime_id: String,
        worker_id: String,
    },
    SubWorker {
        name: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerObservationSubject {
    pub subject: WorkerObservationSubjectRef,
    pub display_name: String,
    pub relation: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct WorkerSessionCapture {
    pub segment_id: String,
    pub session: protocol::SessionSnapshot,
}

impl WorkerSessionCapture {
    pub fn from_log_entries(
        segment_id: impl Into<String>,
        log_entries: &[LogEntry],
    ) -> Result<Self, String> {
        Ok(Self {
            segment_id: segment_id.into(),
            session: session_store::public_snapshot::project_current_session_snapshot(log_entries),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WorkerObservationError {
    #[error("worker session was not found or is not accessible")]
    NotFound,
    #[error("worker session observation failed: {0}")]
    Unavailable(String),
}

#[async_trait]
pub trait WorkerObservationProvider: Send + Sync {
    /// Returns only subjects already authorized for the current Worker.
    async fn list_worker_sessions(
        &self,
    ) -> Result<Vec<WorkerObservationSubject>, WorkerObservationError>;

    /// Reauthorizes and captures the latest committed session for one subject.
    /// Unauthorized and missing subjects must both return `NotFound`.
    async fn capture_worker_session(
        &self,
        subject: &WorkerObservationSubjectRef,
    ) -> Result<WorkerSessionCapture, WorkerObservationError>;
}

pub const TICKET_SERVICE_ID: &str = "ticket.authority";
pub const TICKET_SERVICE_VERSION: &str = "1";

pub trait TicketService: Send + Sync {
    fn ticket_handoff(&self, ticket_ref: &str) -> Result<TicketHandoff, TicketError>;

    /// Typed creation without a synthetic Tool context. The default keeps legacy
    /// handoff-only fixture implementations fail-closed; production installs
    /// provide creation through the shared authorized backend dispatch.
    fn create(&self, _input: NewTicket) -> TicketResult<TicketRef> {
        Err(TicketError::Conflict(
            "Ticket creation service is unavailable".into(),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketHandoff {
    pub id: String,
    pub resource_key: String,
    pub workflow_state: TicketState,
}
#[cfg(test)]
#[path = "contracts_tests.rs"]
mod tests;
