//! Built-in internal feature modules.
//!
//! These modules are compiled into the Worker host and contribute through the
//! same descriptor-approved registry path used by feature modules. They are not
//! an external plugin-loading surface.

pub mod flow_transition;
pub mod manage_workdir;
pub mod manage_worker;
pub mod memory;
pub mod memory_extract;
pub mod merge_request;
pub mod objective;
pub mod orchestration;
pub mod session_explore;
pub mod task;
pub mod ticket;
pub mod worker_observation;

pub(crate) use memory_extract::{MemoryExtractFeature, MemoryExtractState, render_extract_input};
pub(crate) use session_explore::{SessionExploreFeature, SessionExploreState};
pub use task::{TaskFeature, task_tools_feature};
pub use ticket::{
    TicketFeature, TicketFeatureAccess, ticket_tools_feature, ticket_tools_feature_with_access,
    ticket_tools_feature_with_backend,
};
pub use worker_observation::{
    CompositeWorkerObservationProvider, WorkerObservationError, WorkerObservationFeature,
    WorkerObservationProvider, WorkerObservationSubject, WorkerObservationSubjectRef,
    WorkerSessionCapture, WorkspaceClientWorkerObservationProvider,
};
