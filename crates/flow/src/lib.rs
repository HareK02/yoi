//! Declarative Flow definitions and deterministic transition coordination.
//!
//! DCDL is a source format only.  The Flow domain owns the typed source
//! schema, semantic validation, immutable transition snapshots, and state
//! transition rules.

mod builtin;
mod coordinator;
mod definition;
mod selector;

pub use builtin::*;
pub use coordinator::*;
pub use definition::*;
pub use selector::*;
