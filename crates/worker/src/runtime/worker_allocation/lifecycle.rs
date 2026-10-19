//! Owned-allocation guards and the high-level entry points that open
//! the default worker allocation path, mutate it, and return a guard that cleans
//! up on drop.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use manifest::ScopeRule;
use session_store::SegmentId;

use super::error::ScopeLockError;
use super::mutate::release_worker;
use super::table::{LockFileGuard, default_allocation_path};

/// Owned allocation shared with any retained cleanup handles. Only the final
/// owner's drop attempts best-effort release; no `flock` is held for its lifetime.
#[derive(Debug)]
pub struct ScopeAllocationGuard {
    owner: Arc<ScopeAllocationOwner>,
}

/// Retains an allocation's release authority even after its Worker is consumed
/// or dropped. A successful controller transfer should drop this handle without
/// explicitly releasing the live Worker's allocation.
#[derive(Debug, Clone)]
pub struct ScopeAllocationCleanup {
    owner: Arc<ScopeAllocationOwner>,
}

#[derive(Debug)]
struct ScopeAllocationOwner {
    worker_name: String,
    lock_path: PathBuf,
    state: Mutex<ScopeAllocationOwnerState>,
}

#[derive(Debug)]
struct ScopeAllocationOwnerState {
    guard_attached: bool,
    released: bool,
}

impl ScopeAllocationOwner {
    fn release(&self) -> Result<(), ScopeLockError> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.guard_attached {
            return Err(ScopeLockError::AllocationGuardAttached {
                worker_name: self.worker_name.clone(),
            });
        }
        if state.released {
            return Ok(());
        }
        let mut guard = LockFileGuard::open(&self.lock_path)?;
        if guard.data().find(&self.worker_name).is_some() {
            release_worker(&mut guard, &self.worker_name)?;
        } else {
            // A previous save may have removed the entry but failed after rename.
            // Complete the durable save before reporting release as confirmed.
            guard.save()?;
        }
        state.released = true;
        Ok(())
    }
}

impl Drop for ScopeAllocationOwner {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

impl Drop for ScopeAllocationGuard {
    fn drop(&mut self) {
        self.owner
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .guard_attached = false;
    }
}

impl ScopeAllocationCleanup {
    /// Release only after the factory has returned an error and no live
    /// Controller can still use this allocation. An attached guard rejects
    /// release. Success is idempotent across all clones; failure retains the
    /// authority for an explicit retry.
    pub fn release(&self) -> Result<(), ScopeLockError> {
        self.owner.release()
    }
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

/// A failed allocation-table publication together with the still-held machine-wide
/// activation authority. Callers must retain `authority` until recovery or process
/// teardown so restore admission cannot register a competing writer.
pub struct SegmentActivationCommitError {
    source: ScopeLockError,
    authority: SegmentActivationGuard,
}

impl std::fmt::Debug for SegmentActivationCommitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SegmentActivationCommitError")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

impl SegmentActivationCommitError {
    pub fn into_parts(self) -> (ScopeLockError, SegmentActivationGuard) {
        (self.source, self.authority)
    }
}

impl ScopeAllocationGuard {
    fn new(worker_name: String, lock_path: PathBuf) -> Self {
        Self {
            owner: Arc::new(ScopeAllocationOwner {
                worker_name,
                lock_path,
                state: Mutex::new(ScopeAllocationOwnerState {
                    guard_attached: true,
                    released: false,
                }),
            }),
        }
    }

    fn commit_acquisition(self, table: &mut LockFileGuard) -> Result<Self, ScopeLockError> {
        let mut published = false;
        match table.save_with_publication(&mut published) {
            Ok(()) => Ok(self),
            Err(source) => {
                let cleanup = if published {
                    Some(self.cleanup_handle())
                } else {
                    // A staged entry is not ours until rename. Disarm before
                    // detaching so Drop cannot remove a preexisting allocation
                    // (or reopen the table lock still held by this caller).
                    self.owner
                        .state
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .released = true;
                    None
                };
                drop(self);
                match cleanup {
                    Some(cleanup) => Err(ScopeLockError::AcquisitionSaveFailed { source, cleanup }),
                    None => Err(ScopeLockError::Io(source)),
                }
            }
        }
    }

    pub fn worker_name(&self) -> &str {
        &self.owner.worker_name
    }

    pub fn lock_path(&self) -> &Path {
        &self.owner.lock_path
    }

    pub fn cleanup_handle(&self) -> ScopeAllocationCleanup {
        ScopeAllocationCleanup {
            owner: Arc::clone(&self.owner),
        }
    }

    /// Lock the machine-wide table and validate a Segment replacement before
    /// its durable metadata is changed.
    pub fn begin_segment_activation(
        &self,
        expected: SegmentId,
        replacement: SegmentId,
    ) -> Result<SegmentActivationGuard, ScopeLockError> {
        let guard = LockFileGuard::open(self.lock_path())?;
        let actual = guard
            .data()
            .allocations
            .iter()
            .find(|allocation| allocation.worker_name == self.worker_name())
            .and_then(|allocation| allocation.segment_id);

        if actual != Some(expected) {
            return Err(ScopeLockError::SegmentChanged {
                worker_name: self.worker_name().to_owned(),
                expected,
                actual,
            });
        }

        if let Some(existing) = guard.data().allocations.iter().find(|allocation| {
            allocation.segment_id == Some(replacement)
                && allocation.worker_name != self.worker_name()
        }) {
            return Err(ScopeLockError::SegmentConflict {
                segment_id: replacement,
                worker_name: existing.worker_name.clone(),
                socket: existing.socket.clone(),
            });
        }

        Ok(SegmentActivationGuard {
            guard,
            worker_name: self.worker_name().to_owned(),
            expected,
            replacement,
        })
    }
}

impl SegmentActivationGuard {
    /// Persist the replacement Segment while retaining exclusive allocation
    /// authority. This is intentionally consumed so a successful metadata CAS
    /// cannot accidentally be followed by an unlocked update.
    pub fn commit(mut self) -> Result<(), SegmentActivationCommitError> {
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
            return Err(SegmentActivationCommitError {
                source: ScopeLockError::SegmentChanged {
                    worker_name: self.worker_name.clone(),
                    expected: self.expected,
                    actual,
                },
                authority: self,
            });
        };
        allocation.segment_id = Some(self.replacement);
        if let Err(source) = self.guard.save() {
            return Err(SegmentActivationCommitError {
                source: ScopeLockError::Io(source),
                authority: self,
            });
        }
        Ok(())
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
    super::mutate::prepare_worker_with_deny(
        &mut guard,
        worker_name.clone(),
        pid,
        socket,
        scope_allow,
        scope_deny,
        segment_id,
    )?;
    ScopeAllocationGuard::new(worker_name, lock_path).commit_acquisition(&mut guard)
}

#[cfg(test)]
pub(crate) fn install_top_level_at_for_test(
    lock_path: PathBuf,
    worker_name: String,
    pid: u32,
    socket: PathBuf,
    scope_allow: Vec<ScopeRule>,
    segment_id: SegmentId,
) -> Result<ScopeAllocationGuard, ScopeLockError> {
    let mut guard = LockFileGuard::open(&lock_path)?;
    super::mutate::prepare_worker_with_deny(
        &mut guard,
        worker_name.clone(),
        pid,
        socket,
        scope_allow,
        Vec::new(),
        segment_id,
    )?;
    ScopeAllocationGuard::new(worker_name, lock_path).commit_acquisition(&mut guard)
}

/// Take ownership of an existing allocation that was pre-registered by
/// a spawning Worker.
///
/// The spawning flow is two-stage: the spawner calls
/// [`super::delegate_scope`] (with its own pid as a live placeholder,
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
    adopt_allocation_at(lock_path, worker_name, new_pid, segment_id)
}

fn adopt_allocation_at(
    lock_path: PathBuf,
    worker_name: String,
    new_pid: u32,
    segment_id: SegmentId,
) -> Result<ScopeAllocationGuard, ScopeLockError> {
    let mut guard = LockFileGuard::open(&lock_path)?;
    let alloc = guard
        .data_mut()
        .find_mut(&worker_name)
        .ok_or_else(|| ScopeLockError::UnknownWorker(worker_name.clone()))?;
    alloc.pid = new_pid;
    alloc.segment_id = Some(segment_id);
    ScopeAllocationGuard::new(worker_name, lock_path).commit_acquisition(&mut guard)
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
    use super::super::table::{Allocation, LockFile};
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
    fn retained_cleanup_handle_defers_release_until_final_owner_drop() {
        let dir = TempDir::new().unwrap();
        let lock_path = dir.path().join("workers.json");
        let guard = install_top_level_at_for_test(
            lock_path.clone(),
            "retained".into(),
            std::process::id(),
            sock("retained"),
            Vec::new(),
            sid(),
        )
        .unwrap();
        let cleanup = guard.cleanup_handle();
        let retained = cleanup.clone();

        drop(guard);
        drop(cleanup);
        {
            let table = LockFileGuard::open(&lock_path).unwrap();
            assert!(table.data().find("retained").is_some());
        }
        drop(retained);
        let table = LockFileGuard::open(&lock_path).unwrap();
        assert!(table.data().find("retained").is_none());
    }

    #[test]
    fn dropping_cleanup_handle_after_transfer_preserves_live_guard() {
        let dir = TempDir::new().unwrap();
        let lock_path = dir.path().join("workers.json");
        let segment = sid();
        let guard = install_top_level_at_for_test(
            lock_path.clone(),
            "transferred".into(),
            std::process::id(),
            sock("transferred"),
            Vec::new(),
            segment,
        )
        .unwrap();

        drop(guard.cleanup_handle());
        // A successful controller transfer leaves the guard usable, including
        // the existing activation protocol, without retaining a cleanup handle.
        let replacement = sid();
        guard
            .begin_segment_activation(segment, replacement)
            .unwrap()
            .commit()
            .unwrap();
        {
            let table = LockFileGuard::open(&lock_path).unwrap();
            assert_eq!(
                table.data().find("transferred").unwrap().segment_id,
                Some(replacement)
            );
        }
        drop(guard);
        let table = LockFileGuard::open(&lock_path).unwrap();
        assert!(table.data().find("transferred").is_none());
    }

    #[test]
    fn cleanup_rejects_live_guard_then_releases_only_detached_allocation() {
        let dir = TempDir::new().unwrap();
        let lock_path = dir.path().join("workers.json");
        let guard = install_top_level_at_for_test(
            lock_path.clone(),
            "live".into(),
            std::process::id(),
            sock("live"),
            Vec::new(),
            sid(),
        )
        .unwrap();
        let _competitor = install_top_level_at_for_test(
            lock_path.clone(),
            "competitor".into(),
            std::process::id(),
            sock("competitor"),
            Vec::new(),
            sid(),
        )
        .unwrap();
        let cleanup = guard.cleanup_handle();
        assert!(matches!(
            cleanup.release(),
            Err(ScopeLockError::AllocationGuardAttached { worker_name }) if worker_name == "live"
        ));
        drop(cleanup.clone());
        {
            let table = LockFileGuard::open(&lock_path).unwrap();
            assert!(table.data().find("live").is_some());
            assert!(table.data().find("competitor").is_some());
        }

        drop(guard);
        cleanup.release().unwrap();
        cleanup.clone().release().unwrap();
        let table = LockFileGuard::open(&lock_path).unwrap();
        assert!(table.data().find("live").is_none());
        assert!(table.data().find("competitor").is_some());
    }

    #[test]
    fn published_install_failure_retains_cleanup_across_error_drop_and_retry() {
        use super::super::table::{SaveFaultPoint, fail_next_save_at_for_test};

        let dir = TempDir::new().unwrap();
        let lock_path = dir.path().join("workers.json");
        let _competitor = install_top_level_at_for_test(
            lock_path.clone(),
            "competitor".into(),
            std::process::id(),
            sock("competitor"),
            Vec::new(),
            sid(),
        )
        .unwrap();
        let segment = sid();
        fail_next_save_at_for_test(&lock_path, SaveFaultPoint::AfterRename);
        let error = install_top_level_at_for_test(
            lock_path.clone(),
            "failed".into(),
            std::process::id(),
            sock("failed"),
            Vec::new(),
            segment,
        )
        .unwrap_err();
        assert!(matches!(
            &error,
            ScopeLockError::AcquisitionSaveFailed { .. }
        ));
        let cleanup = error.allocation_cleanup_handle().unwrap();
        let retry = error.allocation_cleanup_handle().unwrap();
        drop(error);
        {
            let table = LockFileGuard::open(&lock_path).unwrap();
            assert_eq!(
                table.data().find("failed").unwrap().segment_id,
                Some(segment)
            );
            assert!(table.data().find("competitor").is_some());
        }

        fail_next_save_at_for_test(&lock_path, SaveFaultPoint::BeforeRename);
        assert!(matches!(cleanup.release(), Err(ScopeLockError::Io(_))));
        drop(cleanup);
        {
            let table = LockFileGuard::open(&lock_path).unwrap();
            assert!(table.data().find("failed").is_some());
        }
        retry.release().unwrap();
        let table = LockFileGuard::open(&lock_path).unwrap();
        assert!(table.data().find("failed").is_none());
        assert!(table.data().find("competitor").is_some());
    }

    #[test]
    fn refused_install_has_no_cleanup_authority_over_existing_allocation() {
        let dir = TempDir::new().unwrap();
        let lock_path = dir.path().join("workers.json");
        let segment = sid();
        let _competitor = install_top_level_at_for_test(
            lock_path.clone(),
            "competitor".into(),
            std::process::id(),
            sock("competitor"),
            vec![write_rule("/shared", true)],
            segment,
        )
        .unwrap();
        let cases = [
            ("competitor", sid(), Vec::new()),
            ("other", segment, Vec::new()),
            ("other", sid(), vec![write_rule("/shared", true)]),
        ];
        for (name, requested_segment, scope) in cases {
            let error = install_top_level_at_for_test(
                lock_path.clone(),
                name.into(),
                std::process::id(),
                sock("refused"),
                scope,
                requested_segment,
            )
            .unwrap_err();
            assert!(matches!(
                &error,
                ScopeLockError::DuplicateWorkerName(_)
                    | ScopeLockError::SegmentConflict { .. }
                    | ScopeLockError::WriteConflict { .. }
            ));
            assert!(error.allocation_cleanup_handle().is_none());
            drop(error);
            let table = LockFileGuard::open(&lock_path).unwrap();
            assert_eq!(table.data().allocations.len(), 1);
            assert_eq!(
                table.data().find("competitor").unwrap().socket,
                sock("competitor")
            );
            assert_eq!(
                table.data().find("competitor").unwrap().segment_id,
                Some(segment)
            );
        }
    }

    #[test]
    fn unpublished_install_failure_has_no_cleanup_authority() {
        use super::super::table::{SaveFaultPoint, fail_next_save_at_for_test};

        let dir = TempDir::new().unwrap();
        let lock_path = dir.path().join("workers.json");
        let _competitor = install_top_level_at_for_test(
            lock_path.clone(),
            "competitor".into(),
            std::process::id(),
            sock("competitor"),
            Vec::new(),
            sid(),
        )
        .unwrap();
        fail_next_save_at_for_test(&lock_path, SaveFaultPoint::BeforeRename);
        let error = install_top_level_at_for_test(
            lock_path.clone(),
            "failed".into(),
            std::process::id(),
            sock("failed"),
            Vec::new(),
            sid(),
        )
        .unwrap_err();
        assert!(matches!(&error, ScopeLockError::Io(_)));
        assert!(error.allocation_cleanup_handle().is_none());
        drop(error);
        let table = LockFileGuard::open(&lock_path).unwrap();
        assert!(table.data().find("failed").is_none());
        assert!(table.data().find("competitor").is_some());
    }

    #[test]
    fn published_adoption_failure_retains_cleanup_for_retry() {
        use super::super::table::{SaveFaultPoint, fail_next_save_at_for_test};

        let dir = TempDir::new().unwrap();
        let lock_path = dir.path().join("workers.json");
        {
            let mut table = LockFileGuard::open(&lock_path).unwrap();
            delegate_placeholder(&mut table, "child", std::process::id());
            delegate_placeholder(&mut table, "competitor", std::process::id());
        }
        let segment = sid();
        fail_next_save_at_for_test(&lock_path, SaveFaultPoint::AfterRename);
        let error = adopt_allocation_at(
            lock_path.clone(),
            "child".into(),
            std::process::id(),
            segment,
        )
        .unwrap_err();
        let cleanup = error.allocation_cleanup_handle().unwrap();
        drop(error);
        {
            let table = LockFileGuard::open(&lock_path).unwrap();
            assert_eq!(
                table.data().find("child").unwrap().segment_id,
                Some(segment)
            );
        }
        fail_next_save_at_for_test(&lock_path, SaveFaultPoint::AfterRename);
        assert!(matches!(cleanup.release(), Err(ScopeLockError::Io(_))));
        {
            let table = LockFileGuard::open(&lock_path).unwrap();
            assert!(table.data().find("child").is_none());
            assert!(table.data().find("competitor").is_some());
        }
        cleanup.release().unwrap();
        let table = LockFileGuard::open(&lock_path).unwrap();
        assert!(table.data().find("child").is_none());
        assert!(table.data().find("competitor").is_some());
    }

    #[test]
    fn unpublished_adoption_failure_preserves_placeholder_without_cleanup_authority() {
        use super::super::table::{SaveFaultPoint, fail_next_save_at_for_test};

        let dir = TempDir::new().unwrap();
        let lock_path = dir.path().join("workers.json");
        let placeholder_pid = std::process::id();
        {
            let mut table = LockFileGuard::open(&lock_path).unwrap();
            delegate_placeholder(&mut table, "child", placeholder_pid);
        }
        fail_next_save_at_for_test(&lock_path, SaveFaultPoint::BeforeRename);
        let error = adopt_allocation_at(
            lock_path.clone(),
            "child".into(),
            placeholder_pid.wrapping_add(1),
            sid(),
        )
        .unwrap_err();
        assert!(matches!(&error, ScopeLockError::Io(_)));
        assert!(error.allocation_cleanup_handle().is_none());
        drop(error);
        let table = LockFileGuard::open(&lock_path).unwrap();
        let allocation = table.data().find("child").unwrap();
        assert_eq!(allocation.pid, placeholder_pid);
        assert_eq!(allocation.segment_id, None);
    }

    #[test]
    fn failed_cleanup_release_retains_authority_for_retry() {
        use super::super::table::{SaveFaultPoint, fail_next_save_at_for_test};

        for point in [SaveFaultPoint::BeforeRename, SaveFaultPoint::AfterRename] {
            let dir = TempDir::new().unwrap();
            let lock_path = dir.path().join("workers.json");
            let guard = install_top_level_at_for_test(
                lock_path.clone(),
                "failed".into(),
                std::process::id(),
                sock("failed"),
                vec![read_rule("/shared", true)],
                sid(),
            )
            .unwrap();
            let _other = install_top_level_at_for_test(
                lock_path.clone(),
                "other".into(),
                std::process::id(),
                sock("other"),
                vec![read_rule("/shared", true)],
                sid(),
            )
            .unwrap();
            let cleanup = guard.cleanup_handle();
            let retry = cleanup.clone();
            drop(guard);
            fail_next_save_at_for_test(&lock_path, point);

            assert!(matches!(cleanup.release(), Err(ScopeLockError::Io(_))));
            drop(cleanup);
            {
                let table = LockFileGuard::open(&lock_path).unwrap();
                assert_eq!(
                    table.data().find("failed").is_some(),
                    point == SaveFaultPoint::BeforeRename
                );
                assert!(table.data().find("other").is_some());
            }
            retry.release().unwrap();
            {
                let table = LockFileGuard::open(&lock_path).unwrap();
                assert!(table.data().find("failed").is_none());
                assert!(table.data().find("other").is_some());
            }

            // Success disarms every clone and final Drop, even if the name is
            // subsequently allocated to a different Worker.
            let _replacement = install_top_level_at_for_test(
                lock_path.clone(),
                "failed".into(),
                std::process::id(),
                sock("replacement"),
                vec![read_rule("/shared", true)],
                sid(),
            )
            .unwrap();
            retry.clone().release().unwrap();
            drop(retry);
            let table = LockFileGuard::open(&lock_path).unwrap();
            assert_eq!(
                table.data().find("failed").unwrap().socket,
                sock("replacement")
            );
            assert!(table.data().find("other").is_some());
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
        assert!(err.allocation_cleanup_handle().is_none());
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
    fn failed_segment_activation_returns_locked_recovery_authority() {
        let dir = TempDir::new().unwrap();
        let _sandbox = RuntimeDirSandbox::new(dir.path());
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
        let mut activation = guard
            .begin_segment_activation(old_segment, replacement)
            .unwrap();
        activation
            .guard
            .data_mut()
            .allocations
            .retain(|allocation| allocation.worker_name != "activation");

        let error = activation.commit().unwrap_err();
        let (source, authority) = error.into_parts();
        assert!(matches!(
            source,
            ScopeLockError::SegmentChanged {
                expected,
                actual: None,
                ..
            } if expected == old_segment
        ));
        // The authority is returned rather than dropped on failure. Production
        // stores it on Worker to keep restore admission blocked.
        let (sent, received) = std::sync::mpsc::channel();
        let lookup = std::thread::spawn(move || {
            sent.send(lookup_segment(replacement).unwrap()).unwrap();
        });
        assert!(
            received
                .recv_timeout(std::time::Duration::from_millis(50))
                .is_err()
        );
        drop(authority);
        assert!(
            received
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap()
                .is_none()
        );
        lookup.join().unwrap();
    }

    #[test]
    fn allocation_save_failure_preserves_source_table_and_returns_authority() {
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
        let activation = guard
            .begin_segment_activation(old_segment, replacement)
            .unwrap();
        crate::runtime::worker_allocation::table::fail_next_save_for_test(&lock_path);

        let error = activation.commit().unwrap_err();
        let (source, authority) = error.into_parts();
        assert!(matches!(source, ScopeLockError::Io(_)));
        let persisted: LockFile =
            serde_json::from_slice(&std::fs::read(&lock_path).unwrap()).unwrap();
        assert_eq!(
            persisted
                .find("activation")
                .and_then(|allocation| allocation.segment_id),
            Some(old_segment)
        );
        drop(authority);
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
