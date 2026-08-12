//! Owned-allocation guards and the high-level entry points that open
//! the default worker allocation path, mutate it, and return a guard that cleans
//! up on drop.

use std::path::{Path, PathBuf};

use manifest::ScopeRule;
use session_store::SegmentId;

use super::error::ScopeLockError;
use super::mutate::release_worker;
use super::table::{LockFileGuard, default_allocation_path};

/// Owned allocation: on drop, opens the lock file and releases this
/// Worker's entry. The guard keeps only the name + lock-file path; it
/// does not hold the `flock` for the Worker's lifetime.
#[derive(Debug)]
pub struct ScopeAllocationGuard {
    worker_name: String,
    lock_path: PathBuf,
}

impl ScopeAllocationGuard {
    pub fn worker_name(&self) -> &str {
        &self.worker_name
    }

    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }
}

impl Drop for ScopeAllocationGuard {
    fn drop(&mut self) {
        if let Ok(mut guard) = LockFileGuard::open(&self.lock_path) {
            let _ = release_worker(&mut guard, &self.worker_name);
        }
    }
}

/// Open the default lock file, register a top-level Worker, and return a
/// guard that will release the allocation on drop.
pub fn install_top_level(
    worker_name: String,
    pid: u32,
    socket: PathBuf,
    scope_allow: Vec<ScopeRule>,
    segment_id: SegmentId,
) -> Result<ScopeAllocationGuard, ScopeLockError> {
    install_top_level_with_deny(
        worker_name,
        pid,
        socket,
        scope_allow,
        Vec::new(),
        segment_id,
    )
}

/// Open the default lock file, register a top-level Worker with explicit
/// deny rules, and return a guard that will release the allocation on
/// drop.
pub fn install_top_level_with_deny(
    worker_name: String,
    pid: u32,
    socket: PathBuf,
    scope_allow: Vec<ScopeRule>,
    scope_deny: Vec<ScopeRule>,
    segment_id: SegmentId,
) -> Result<ScopeAllocationGuard, ScopeLockError> {
    let lock_path = default_allocation_path()?;
    let mut guard = LockFileGuard::open(&lock_path)?;
    super::mutate::register_worker_with_deny(
        &mut guard,
        worker_name.clone(),
        pid,
        socket,
        scope_allow,
        scope_deny,
        segment_id,
    )?;
    Ok(ScopeAllocationGuard {
        worker_name,
        lock_path,
    })
}

/// Take ownership of an existing allocation that was pre-registered by
/// a spawning Worker.
///
/// The spawning flow is two-stage: the spawner calls
/// [`crate::delegate_scope`] (with its own pid as a live placeholder,
/// `segment_id = None`), then exec's the child; the child, once
/// running, calls this function to rewrite the allocation's pid +
/// segment_id to its own and claim the [`ScopeAllocationGuard`] so
/// the entry is released when the child exits.
pub fn adopt_allocation(
    worker_name: String,
    new_pid: u32,
    segment_id: SegmentId,
) -> Result<ScopeAllocationGuard, ScopeLockError> {
    let lock_path = default_allocation_path()?;
    let mut guard = LockFileGuard::open(&lock_path)?;
    let alloc = guard
        .data_mut()
        .find_mut(&worker_name)
        .ok_or_else(|| ScopeLockError::UnknownWorker(worker_name.clone()))?;
    alloc.pid = new_pid;
    alloc.segment_id = Some(segment_id);
    guard.save()?;
    Ok(ScopeAllocationGuard {
        worker_name,
        lock_path,
    })
}

/// Rewrite the `segment_id` recorded for `worker_name` to
/// `new_segment_id`.
///
/// The Worker's in-memory `segment_id` can change underneath the
/// allocation in two normal places:
///
/// - `Worker::compact` mints a fresh Segment in the same Session.
/// - `session_store::ensure_head_or_fork` auto-forks within that Session when another
///   writer has advanced the store head behind our back.
///
/// Both paths must call this so subsequent [`lookup_segment`] queries
/// find the live Segment id, not the old one. Without this update a
/// concurrent `restore_from_manifest(new_id)` would see "no live
/// writer" and proceed to register a competing allocation on the
/// Segment lineage this Worker just moved into.
///
/// The lock is opened once and the allocation is rewritten inside the
/// guard, so the segment_id collision check is atomic with the
/// rewrite.
pub fn update_segment(worker_name: &str, new_segment_id: SegmentId) -> Result<(), ScopeLockError> {
    let lock_path = default_allocation_path()?;
    let mut guard = LockFileGuard::open(&lock_path)?;
    if let Some(other) = guard.data().find_by_segment(new_segment_id) {
        if other.worker_name != worker_name {
            return Err(ScopeLockError::SegmentConflict {
                segment_id: new_segment_id,
                worker_name: other.worker_name.clone(),
                socket: other.socket.clone(),
            });
        }
    }
    let alloc = guard
        .data_mut()
        .find_mut(worker_name)
        .ok_or_else(|| ScopeLockError::UnknownWorker(worker_name.into()))?;
    alloc.segment_id = Some(new_segment_id);
    guard.save()?;
    Ok(())
}

/// Information about a Worker that currently holds an allocation for a
/// given session.
#[derive(Debug, Clone)]
pub struct SegmentLockInfo {
    pub worker_name: String,
    pub socket: PathBuf,
    pub pid: u32,
}

/// Open the default lock file, reclaim stale entries, and return the
/// allocation currently writing to `segment_id`, if any.
///
/// Used by `Worker::restore_from_manifest` to refuse a resume that would
/// race a live writer on the same source session.
pub fn lookup_segment(segment_id: SegmentId) -> Result<Option<SegmentLockInfo>, ScopeLockError> {
    let lock_path = default_allocation_path()?;
    let mut guard = LockFileGuard::open(&lock_path)?;
    super::mutate::reclaim_stale(&mut guard);
    Ok(guard
        .data()
        .find_by_segment(segment_id)
        .map(|a| SegmentLockInfo {
            worker_name: a.worker_name.clone(),
            socket: a.socket.clone(),
            pid: a.pid,
        }))
}

#[cfg(test)]
mod tests {
    use super::super::table::Allocation;
    use super::super::test_util::*;
    use super::*;
    use tempfile::TempDir;

    /// Mimic what the spawner does before the child comes up: push an
    /// allocation for the child carrying the spawner's (live) pid as a
    /// placeholder. Exists only in tests.
    fn delegate_placeholder(g: &mut LockFileGuard, worker_name: &str, placeholder_pid: u32) {
        g.data_mut().allocations.push(Allocation {
            worker_name: worker_name.to_string(),
            pid: placeholder_pid,
            socket: sock(worker_name),
            scope_allow: vec![write_rule("/tmp/child", true)],
            scope_deny: Vec::new(),
            delegated_from: None,
            segment_id: None,
        });
        g.save().unwrap();
    }

    #[test]
    fn scope_allocation_guard_releases_on_drop() {
        let dir = TempDir::new().unwrap();
        let _sandbox = RuntimeDirSandbox::new(dir.path());
        let lock_path = dir.path().join("workers.json");
        let guard = install_top_level(
            "a".into(),
            std::process::id(),
            sock("a"),
            vec![write_rule("/src", true)],
            sid(),
        )
        .unwrap();
        {
            let g = LockFileGuard::open(&lock_path).unwrap();
            assert!(g.data().find("a").is_some());
        }
        drop(guard);
        {
            let g = LockFileGuard::open(&lock_path).unwrap();
            assert!(g.data().find("a").is_none());
        }
    }

    #[test]
    fn adopt_allocation_rewrites_pid_and_releases_on_drop() {
        let dir = TempDir::new().unwrap();
        let _sandbox = RuntimeDirSandbox::new(dir.path());
        let lock_path = dir.path().join("workers.json");
        // Pre-register an allocation under spawner's pid, as delegate_scope would.
        {
            let mut g = LockFileGuard::open(&lock_path).unwrap();
            delegate_placeholder(&mut g, "child", std::process::id());
        }
        let child_pid = std::process::id().wrapping_add(1);
        let guard = adopt_allocation("child".into(), child_pid, sid()).unwrap();
        {
            let g = LockFileGuard::open(&lock_path).unwrap();
            let alloc = g.data().find("child").unwrap();
            assert_eq!(alloc.pid, child_pid);
        }
        drop(guard);
        {
            let g = LockFileGuard::open(&lock_path).unwrap();
            assert!(g.data().find("child").is_none());
        }
    }

    #[test]
    fn adopt_allocation_errors_on_unknown_pod() {
        let dir = TempDir::new().unwrap();
        let _sandbox = RuntimeDirSandbox::new(dir.path());
        let err = adopt_allocation("ghost".into(), 42, sid()).unwrap_err();
        assert!(matches!(err, ScopeLockError::UnknownWorker(ref n) if n == "ghost"));
    }

    #[test]
    fn lookup_session_returns_live_writer_info() {
        let dir = TempDir::new().unwrap();
        let _sandbox = RuntimeDirSandbox::new(dir.path());
        let s = sid();
        let guard = install_top_level(
            "live".into(),
            std::process::id(),
            sock("live"),
            vec![write_rule("/work", true)],
            s,
        )
        .unwrap();
        let info = lookup_segment(s).unwrap().expect("expected live writer");
        assert_eq!(info.worker_name, "live");
        assert_eq!(info.socket, sock("live"));
        drop(guard);
        // After the guard's release, the lookup goes back to None.
        assert!(lookup_segment(s).unwrap().is_none());
    }

    #[test]
    fn update_session_rewrites_allocation_session_id() {
        let dir = TempDir::new().unwrap();
        let _sandbox = RuntimeDirSandbox::new(dir.path());
        let original = sid();
        let updated = sid();
        let _guard = install_top_level(
            "p".into(),
            std::process::id(),
            sock("p"),
            vec![write_rule("/work", true)],
            original,
        )
        .unwrap();
        update_segment("p", updated).unwrap();
        // lookup against the original is now empty, the updated id wins.
        assert!(lookup_segment(original).unwrap().is_none());
        assert_eq!(lookup_segment(updated).unwrap().unwrap().worker_name, "p");
    }

    #[test]
    fn update_session_rejects_when_target_already_held() {
        let dir = TempDir::new().unwrap();
        let _sandbox = RuntimeDirSandbox::new(dir.path());
        let s_a = sid();
        let s_b = sid();
        let _g_a = install_top_level(
            "a".into(),
            std::process::id(),
            sock("a"),
            vec![write_rule("/work/a", true)],
            s_a,
        )
        .unwrap();
        let _g_b = install_top_level(
            "b".into(),
            std::process::id(),
            sock("b"),
            vec![write_rule("/work/b", true)],
            s_b,
        )
        .unwrap();
        // `a` cannot adopt b's live session id.
        let err = update_segment("a", s_b).unwrap_err();
        match err {
            ScopeLockError::SegmentConflict {
                worker_name,
                segment_id,
                ..
            } => {
                assert_eq!(worker_name, "b");
                assert_eq!(segment_id, s_b);
            }
            other => panic!("expected SegmentConflict, got {other:?}"),
        }
    }
}
