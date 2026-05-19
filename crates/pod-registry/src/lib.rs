//! Machine-wide Pod allocation registry.
//!
//! A single JSON file at `<runtime_dir>/pods.json` records every live
//! Pod's allocation (see [`manifest::paths::pod_registry_path`] for
//! how the path is resolved). File-level `flock(2)` serialises access
//! across processes so spawn sequences from unrelated Pods can't race.
//!
//! Each Pod, when starting, acquires the lock, reclaims stale entries
//! (Pods whose PID has died), checks that its requested write scope
//! does not overlap any other allocation's effective write scope, and
//! registers itself. When it exits normally, it removes its entry and
//! returns delegated scope to its `delegated_from` parent. Crash
//! recovery rides on the next Pod that opens the file — no background
//! reaper.

mod conflict;
mod error;
mod lifecycle;
mod mutate;
mod table;

#[cfg(test)]
mod test_util;

pub use conflict::{
    ConflictOwner, find_conflict_owner, find_conflict_owners, is_within_effective_write,
};
pub use error::ScopeLockError;
pub use lifecycle::{
    ScopeAllocationGuard, SegmentLockInfo, adopt_allocation, install_top_level,
    install_top_level_with_deny, lookup_segment, update_segment,
};
pub use mutate::{
    delegate_scope, reclaim_stale, reclaim_stale_with, register_pod, register_pod_with_deny,
    release_pod,
};
pub use table::{Allocation, LockFile, LockFileGuard, default_registry_path};
