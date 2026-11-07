use std::sync::atomic::{AtomicU64, Ordering};

/// Read-only observations for one registration (and its clones) across its scopes.
/// Counts real I/O boundaries, not API opens, and retains no physical database.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpenObservation {
    pub physical_open_attempts: u64,
    pub physical_opens: u64,
    pub completed_initializations: u64,
    pub directory_sync_attempts: u64,
    pub directory_sync_successes: u64,
}

impl OpenObservation {
    pub fn since(self, before: Self) -> Self {
        Self {
            physical_open_attempts: self.physical_open_attempts - before.physical_open_attempts,
            physical_opens: self.physical_opens - before.physical_opens,
            completed_initializations: self.completed_initializations
                - before.completed_initializations,
            directory_sync_attempts: self.directory_sync_attempts - before.directory_sync_attempts,
            directory_sync_successes: self.directory_sync_successes
                - before.directory_sync_successes,
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct Counters {
    physical_open_attempts: AtomicU64,
    physical_opens: AtomicU64,
    completed_initializations: AtomicU64,
    directory_sync_attempts: AtomicU64,
    directory_sync_successes: AtomicU64,
}

impl Counters {
    pub(super) fn snapshot(&self) -> OpenObservation {
        OpenObservation {
            physical_open_attempts: self.physical_open_attempts.load(Ordering::Relaxed),
            physical_opens: self.physical_opens.load(Ordering::Relaxed),
            completed_initializations: self.completed_initializations.load(Ordering::Relaxed),
            directory_sync_attempts: self.directory_sync_attempts.load(Ordering::Relaxed),
            directory_sync_successes: self.directory_sync_successes.load(Ordering::Relaxed),
        }
    }
    pub(super) fn open_attempt(&self) {
        self.physical_open_attempts.fetch_add(1, Ordering::Relaxed);
    }
    pub(super) fn open_success(&self) {
        self.physical_opens.fetch_add(1, Ordering::Relaxed);
    }
    pub(super) fn initialized(&self) {
        self.completed_initializations
            .fetch_add(1, Ordering::Relaxed);
    }
    pub(super) fn sync_attempt(&self) {
        self.directory_sync_attempts.fetch_add(1, Ordering::Relaxed);
    }
    pub(super) fn sync_success(&self) {
        self.directory_sync_successes
            .fetch_add(1, Ordering::Relaxed);
    }
}
