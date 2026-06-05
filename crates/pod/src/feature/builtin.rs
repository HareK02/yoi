//! Built-in internal feature modules.
//!
//! These modules are compiled into the Pod host and contribute through the
//! same descriptor-approved registry path used by feature modules. They are not
//! an external plugin-loading surface.

pub mod task;

pub use task::task_tools_feature;
