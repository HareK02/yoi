//! Built-in internal feature modules.
//!
//! These modules are compiled into the Worker host and contribute through the
//! same descriptor-approved registry path used by feature modules. They are not
//! an external plugin-loading surface.

mod backend_diagnostic;
pub(crate) mod backend_job_input;
pub mod backend_job_result;
pub(crate) mod bash;

/// Host metadata participants are independent of model-facing Tool enablement.
/// Disabling a Feature must not preserve stale Session-bound authority evidence.
pub(crate) fn session_metadata_lifecycles()
-> Vec<std::sync::Arc<dyn crate::metadata_lifecycle::SessionMetadataLifecycle>> {
    vec![std::sync::Arc::new(
        crate::subjektiv_metadata::SessionAttributionMetadata,
    )]
}

pub mod chat_invocation;
pub mod drive;
pub mod flow_transition;
pub mod journal;
pub mod manage_workdir;
pub mod manage_worker;
pub mod memory;
pub(crate) mod memory_lifecycle;
pub mod memory_staging_output;
pub(crate) mod memory_surface_lifecycle;
pub(crate) mod memory_surface_output;
pub mod merge_request;
pub mod objective;
pub mod orchestration;
mod resource_projection;
pub mod session_explore;
pub(crate) mod subjektiv_memory;
pub(crate) mod subjektiv_session;
pub mod task;
pub mod ticket;
pub(crate) mod web;
pub mod worker_observation;
pub mod workspace_config;
pub mod workspace_worker_discovery;

pub use task::{TaskFeature, task_tools_feature};
pub use ticket::{TicketFeature, TicketFeatureAccess, ticket_tools_feature};
pub use worker_observation::{
    CompositeWorkerObservationProvider, WorkerObservationError, WorkerObservationFeature,
    WorkerObservationProvider, WorkerObservationSubject, WorkerObservationSubjectRef,
    WorkerSessionCapture, WorkspaceClientWorkerObservationProvider,
};
