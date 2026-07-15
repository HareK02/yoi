//! Process-local Worker allocation table used only for scope ownership checks.
//!
//! This module is intentionally not a runtime identity store. Runtime Worker
//! identity, creation and durable persistence remain owned by worker-runtime
//! fs-store plus its execution backend mapping; this table coordinates
//! in-process scope delegation while a Worker is running.

mod conflict;
mod error;
mod lifecycle;
mod mutate;
mod table;

#[cfg(test)]
pub(crate) mod test_util;

pub use conflict::{
    ConflictOwner, find_conflict_owner, find_conflict_owners, is_within_effective_write,
};
pub use error::ScopeLockError;
pub use lifecycle::{
    ScopeAllocationGuard, SegmentLockInfo, adopt_allocation, install_top_level,
    install_top_level_with_deny, lookup_segment, update_segment,
};
pub use mutate::{
    delegate_scope, reclaim_delegated_scope, reclaim_stale, reclaim_stale_with, register_worker,
    register_worker_with_deny, release_worker,
};
pub use table::{Allocation, LockFile, LockFileGuard, default_allocation_path};
