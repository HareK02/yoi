//! Error type for mutating Worker allocation operations.

use std::io;
use std::path::PathBuf;

use manifest::{ScopeError, ScopeRule};
use session_store::SegmentId;

/// Errors raised by the mutating Worker allocation operations.
#[derive(Debug, thiserror::Error)]
pub enum ScopeLockError {
    #[error("I/O error on workers.json: {0}")]
    Io(#[from] io::Error),
    #[error("worker `{worker_name}` allocation still has a live guard")]
    AllocationGuardAttached { worker_name: String },
    #[error("allocation acquisition was published but its save failed: {source}")]
    AcquisitionSaveFailed {
        #[source]
        source: io::Error,
        cleanup: super::lifecycle::ScopeAllocationCleanup,
    },
    #[error("worker `{0}` is already allocated")]
    DuplicateWorkerName(String),
    #[error("requested scope `{}` conflicts with worker allocation `{competitor}` rule `{}`", .rule.target.display(), .competitor_rule.target.display())]
    WriteConflict {
        competitor: String,
        rule: ScopeRule,
        competitor_rule: ScopeRule,
    },
    #[error(
        "requested scope `{}` is not within spawner `{spawner}`'s delegation scope",
        .rule.target.display()
    )]
    NotSubset { spawner: String, rule: ScopeRule },
    #[error("invalid delegation scope: {source}")]
    InvalidScope { source: ScopeError },
    #[error("worker `{0}` is not allocated")]
    UnknownWorker(String),
    #[error(
        "session {segment_id} is already allocated to worker `{worker_name}` at {}",
        .socket.display()
    )]
    SegmentConflict {
        segment_id: SegmentId,
        worker_name: String,
        socket: PathBuf,
    },
    #[error(
        "worker `{worker_name}` allocation moved from expected Segment {expected} to {actual:?}"
    )]
    SegmentChanged {
        worker_name: String,
        expected: SegmentId,
        actual: Option<SegmentId>,
    },
}

impl ScopeLockError {
    /// Retain the release authority for an owned acquisition whose table rename
    /// succeeded before save returned an error. Refusals and unpublished saves
    /// do not confer ownership of any preexisting allocation.
    pub fn allocation_cleanup_handle(&self) -> Option<super::lifecycle::ScopeAllocationCleanup> {
        match self {
            Self::AcquisitionSaveFailed { cleanup, .. } => Some(cleanup.clone()),
            _ => None,
        }
    }
}
