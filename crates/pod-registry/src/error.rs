//! Error type for mutating pod-registry operations.

use std::io;
use std::path::PathBuf;

use manifest::{ScopeError, ScopeRule};
use session_store::SegmentId;

/// Errors raised by the mutating pod-registry operations.
#[derive(Debug, thiserror::Error)]
pub enum ScopeLockError {
    #[error("I/O error on workers.json: {0}")]
    Io(#[from] io::Error),
    #[error("pod name `{0}` is already registered")]
    DuplicateWorkerName(String),
    #[error("requested scope `{}` conflicts with pod `{competitor}` rule `{}`", .rule.target.display(), .competitor_rule.target.display())]
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
    #[error("pod `{0}` is not registered")]
    UnknownWorker(String),
    #[error(
        "session {segment_id} is already held by pod `{worker_name}` at {}",
        .socket.display()
    )]
    SegmentConflict {
        segment_id: SegmentId,
        worker_name: String,
        socket: PathBuf,
    },
}
