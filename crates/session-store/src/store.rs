//! Persistence backend abstraction.
//!
//! [`Store`] defines the sync interface for reading and writing segment logs.
//! Implementations handle the physical storage (filesystem, database, etc.).
//!
//! Sync (rather than async) is intentional: a segment log append is a single
//! `< 1 KiB` line on local fs and completes well below a millisecond. Going
//! through `tokio::fs` would force every caller — including `Worker`'s sync
//! `on_history_append` callback — to bridge sync → async via a channel +
//! drain task. Keeping the store sync lets the worker callback, Pod commit
//! paths, and `PodInterceptor` all share one direct `append_entry` call.

use crate::SegmentId;
use crate::event_trace::TraceEntry;
use crate::segment_log::LogEntry;

/// Errors from the persistence store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("segment not found: {0}")]
    NotFound(SegmentId),

    #[error("log corrupted at line {line}: {message}")]
    Corrupt { line: usize, message: String },
}

/// Sync persistence backend for segment logs.
///
/// All methods take `&self` — implementations should use interior mutability
/// (e.g., append-mode file handles) when needed.
pub trait Store: Send + Sync {
    /// Append a single log entry to the segment log.
    ///
    /// One line per call. The kernel orders concurrent `O_APPEND` writes
    /// for lines < `PIPE_BUF`, so user-space serialization is unnecessary.
    fn append(&self, id: SegmentId, entry: &LogEntry) -> Result<(), StoreError>;

    /// Read all log entries for a segment, in order.
    fn read_all(&self, id: SegmentId) -> Result<Vec<LogEntry>, StoreError>;

    /// List all segment IDs, most recent first.
    fn list_segments(&self) -> Result<Vec<SegmentId>, StoreError>;

    /// Create a new segment with initial entries.
    fn create_segment(&self, id: SegmentId, entries: &[LogEntry]) -> Result<(), StoreError>;

    /// Check if a segment exists.
    fn exists(&self, id: SegmentId) -> Result<bool, StoreError>;

    /// Count entries currently stored for a segment.
    ///
    /// Used by `ensure_head_or_fork` to detect concurrent writers:
    /// if the on-disk count exceeds the writer's own append tally,
    /// another process has extended the log.
    fn read_entry_count(&self, id: SegmentId) -> Result<usize, StoreError>;

    /// Append a trace entry to the debug event trace file.
    fn append_trace(&self, id: SegmentId, entry: &TraceEntry) -> Result<(), StoreError>;
}
