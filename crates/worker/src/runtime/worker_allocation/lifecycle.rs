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

/// Exclusive authority to activate a replacement Segment for one allocated Worker.
///
/// The allocation lock remains held across the durable metadata CAS and the final
/// allocation-table update, so restore admission cannot observe an intermediate
/// ownership state.
pub struct SegmentActivationGuard {
    guard: LockFileGuard,
    worker_name: String,
    expected: SegmentId,
    replacement: SegmentId,
}

impl ScopeAllocationGuard {
    pub fn worker_name(&self) -> &str {
        &self.worker_name
    }

    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    /// Lock the machine-wide table and validate a Segment replacement before
    /// its durable metadata is changed.
    pub fn begin_segment_activation(
        &self,
        expected: SegmentId,
        replacement: SegmentId,
    ) -> Result<SegmentActivationGuard, ScopeLockError> {
        let guard = LockFileGuard::open(&self.lock_path)?;
        let actual = guard
            .data()
            .allocations
            .iter()
            .find(|allocation| allocation.worker_name == self.worker_name)
            .and_then(|allocation| allocation.segment_id);

        if actual != Some(expected) {
            return Err(ScopeLockError::SegmentChanged {
                worker_name: self.worker_name.clone(),
                expected,
                actual,
            });
        }

        if let Some(existing) = guard.data().allocations.iter().find(|allocation| {
            allocation.segment_id == Some(replacement)
                && allocation.worker_name != self.worker_name
        }) {
            return Err(ScopeLockError::SegmentConflict {
                segment_id: replacement,
                worker_name: existing.worker_name.clone(),
                socket: existing.socket.clone(),
            });
        }

        Ok(SegmentActivationGuard {
            guard,
            worker_name: self.worker_name.clone(),
            expected,
            replacement,
        })
    }
}

impl SegmentActivationGuard {
    /// Persist the replacement Segment while retaining exclusive allocation
    /// authority. This is intentionally consumed so a successful metadata CAS
    /// cannot accidentally be followed by an unlocked update.
    pub fn commit(mut self) -> Result<(), ScopeLockError> {
        let actual = self
            .guard
            .data()
            .allocations
            .iter()
            .find(|allocation| allocation.worker_name == self.worker_name)
            .and_then(|allocation| allocation.segment_id);
        let Some(allocation) = self
            .guard
            .data_mut()
            .allocations
            .iter_mut()
            .find(|allocation| allocation.worker_name == self.worker_name)
        else {
            return Err(ScopeLockError::SegmentChanged {
                worker_name: self.worker_name,
                expected: self.expected,
                actual,
            });
        };
        allocation.segment_id = Some(self.replacement);
        self.guard.save()?;
        Ok(())
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
    fn segment_activation_rejects_when_target_already_held() {
        let dir = TempDir::new().unwrap();
        let _sandbox = RuntimeDirSandbox::new(dir.path());
        let s_a = sid();
        let s_b = sid();
        let g_a = install_top_level(
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
        // `a` cannot activate b's live Segment id.
        let err = match g_a.begin_segment_activation(s_a, s_b) {
            Ok(_) => panic!("expected SegmentConflict"),
            Err(error) => error,
        };
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

    #[test]
    fn segment_activation_commits_verified_replacement() {
        let dir = TempDir::new().unwrap();
        let _sandbox = RuntimeDirSandbox::new(dir.path());
        let lock_path = dir.path().join("workers.json");
        let old_segment = sid();
        let replacement = sid();
        let guard = install_top_level(
            "activation".into(),
            std::process::id(),
            sock("activation"),
            vec![write_rule("/work", true)],
            old_segment,
        )
        .unwrap();

        guard
            .begin_segment_activation(old_segment, replacement)
            .unwrap()
            .commit()
            .unwrap();

        let table = LockFileGuard::open(&lock_path).unwrap();
        assert_eq!(
            table
                .data()
                .find("activation")
                .and_then(|allocation| allocation.segment_id),
            Some(replacement)
        );
    }

    #[test]
    fn dropping_segment_activation_preserves_source_allocation() {
        let dir = TempDir::new().unwrap();
        let _sandbox = RuntimeDirSandbox::new(dir.path());
        let lock_path = dir.path().join("workers.json");
        let old_segment = sid();
        let replacement = sid();
        let guard = install_top_level(
            "activation".into(),
            std::process::id(),
            sock("activation"),
            vec![write_rule("/work", true)],
            old_segment,
        )
        .unwrap();

        drop(
            guard
                .begin_segment_activation(old_segment, replacement)
                .unwrap(),
        );

        let table = LockFileGuard::open(&lock_path).unwrap();
        assert_eq!(
            table
                .data()
                .find("activation")
                .and_then(|allocation| allocation.segment_id),
            Some(old_segment)
        );
    }

    #[test]
    fn segment_activation_rejects_stale_expected_segment_without_mutation() {
        let dir = TempDir::new().unwrap();
        let _sandbox = RuntimeDirSandbox::new(dir.path());
        let lock_path = dir.path().join("workers.json");
        let allocated = sid();
        let stale = sid();
        let replacement = sid();
        let guard = install_top_level(
            "activation".into(),
            std::process::id(),
            sock("activation"),
            vec![write_rule("/work", true)],
            allocated,
        )
        .unwrap();

        assert!(matches!(
            guard.begin_segment_activation(stale, replacement),
            Err(ScopeLockError::SegmentChanged {
                expected,
                actual: Some(actual),
                ..
            }) if expected == stale && actual == allocated
        ));
        let table = LockFileGuard::open(&lock_path).unwrap();
        assert_eq!(
            table
                .data()
                .find("activation")
                .and_then(|allocation| allocation.segment_id),
            Some(allocated)
        );
    }
}
