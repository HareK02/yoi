use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Cumulative lifecycle observations for one storage manager and its clones/handles.
/// Counts attempts before starting I/O, including attempts that return errors.
/// `acquisitions` counts entries to lifecycle acquisition; `sync_attempts` counts
/// lifecycle durability establishment attempts (the full ancestor sync plus
/// lockfile and lock-directory persistence), and `lock_attempts` counts OS
/// shared/exclusive lock calls, excluding lockfile open failures. Once this
/// manager has established and rechecked the same physical paths, ordinary
/// acquisitions need no establishment attempt. A fresh manager establishes again.
///
/// Durations sum monotonic elapsed nanoseconds around those calls, including
/// errors and OS lock waits, but not lease holding time or database operations.
/// Overlapping threads contribute independently: aggregates are not wall time.
/// Fields saturate at u64::MAX and are independently sampled with relaxed loads;
/// a concurrent snapshot need not describe one atomic point in time. In-flight
/// calls contribute attempts before their duration or failure is recorded.
/// These diagnostics retain no database or authority and are not termination,
/// durability, or successful-work evidence; they never control storage I/O.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LifecycleObservation {
    pub acquisitions: u64,
    pub sync_attempts: u64,
    pub sync_nanoseconds: u64,
    pub failed_syncs: u64,
    pub lock_attempts: u64,
    pub lock_nanoseconds: u64,
    pub failed_locks: u64,
}

impl LifecycleObservation {
    /// Saturating differences between snapshots from the same manager.
    pub fn since(self, before: Self) -> Self {
        Self {
            acquisitions: self.acquisitions.saturating_sub(before.acquisitions),
            sync_attempts: self.sync_attempts.saturating_sub(before.sync_attempts),
            sync_nanoseconds: self
                .sync_nanoseconds
                .saturating_sub(before.sync_nanoseconds),
            failed_syncs: self.failed_syncs.saturating_sub(before.failed_syncs),
            lock_attempts: self.lock_attempts.saturating_sub(before.lock_attempts),
            lock_nanoseconds: self
                .lock_nanoseconds
                .saturating_sub(before.lock_nanoseconds),
            failed_locks: self.failed_locks.saturating_sub(before.failed_locks),
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct Counters {
    acquisitions: AtomicU64,
    sync_attempts: AtomicU64,
    sync_nanoseconds: AtomicU64,
    failed_syncs: AtomicU64,
    lock_attempts: AtomicU64,
    lock_nanoseconds: AtomicU64,
    failed_locks: AtomicU64,
}

impl Counters {
    pub(super) fn snapshot(&self) -> LifecycleObservation {
        LifecycleObservation {
            acquisitions: self.acquisitions.load(Ordering::Relaxed),
            sync_attempts: self.sync_attempts.load(Ordering::Relaxed),
            sync_nanoseconds: self.sync_nanoseconds.load(Ordering::Relaxed),
            failed_syncs: self.failed_syncs.load(Ordering::Relaxed),
            lock_attempts: self.lock_attempts.load(Ordering::Relaxed),
            lock_nanoseconds: self.lock_nanoseconds.load(Ordering::Relaxed),
            failed_locks: self.failed_locks.load(Ordering::Relaxed),
        }
    }

    pub(super) fn acquisition(&self) {
        add(&self.acquisitions, 1);
    }

    pub(super) fn sync_start(&self) -> Instant {
        add(&self.sync_attempts, 1);
        Instant::now()
    }

    pub(super) fn sync_finished(&self, started: Instant, failed: bool) {
        let elapsed = started.elapsed();
        add(&self.sync_nanoseconds, nanoseconds(elapsed));
        if failed {
            add(&self.failed_syncs, 1);
        }
    }

    pub(super) fn lock_start(&self) -> Instant {
        add(&self.lock_attempts, 1);
        Instant::now()
    }

    pub(super) fn lock_finished(&self, started: Instant, failed: bool) {
        let elapsed = started.elapsed();
        add(&self.lock_nanoseconds, nanoseconds(elapsed));
        if failed {
            add(&self.failed_locks, 1);
        }
    }
}

fn nanoseconds(duration: std::time::Duration) -> u64 {
    duration.as_nanos().min(u128::from(u64::MAX)) as u64
}

fn add(counter: &AtomicU64, amount: u64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(value.saturating_add(amount))
    });
}
