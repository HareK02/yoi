//! Built-in internal feature modules.
//!
//! These modules are compiled into the Worker host and contribute through the
//! same descriptor-approved registry path used by feature modules. They are not
//! an external plugin-loading surface.

pub mod session_explore;
pub mod task;
pub mod ticket;

pub(crate) use session_explore::{
    SessionExploreFeature, SessionExploreState, render_extract_input,
};
pub use task::{TaskFeature, task_tools_feature};
pub use ticket::{
    TicketFeature, TicketFeatureAccess, ticket_tools_feature, ticket_tools_feature_with_access,
    ticket_tools_feature_with_backend,
};
