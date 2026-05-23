//! Persistence backend abstraction.
//!
//! [`Store`] defines the sync interface for reading and writing segment logs
//! within a [`Session`](crate::SessionId). Implementations handle the
//! physical storage (filesystem, database, etc.).
//!
//! Sync (rather than async) is intentional: a segment log append is a single
//! `< 1 KiB` line on local fs and completes well below a millisecond. Going
//! through `tokio::fs` would force every caller — including `Worker`'s sync
//! `on_history_append` callback — to bridge sync → async via a channel +
//! drain task. Keeping the store sync lets the worker callback, Pod commit
//! paths, and `PodInterceptor` all share one direct `append_entry` call.

use crate::event_trace::TraceEntry;
use crate::segment_log::LogEntry;
use crate::{SegmentId, SessionId};

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

    #[error("invalid pod name: {0}")]
    InvalidPodName(String),
}

/// Sync persistence backend for segment logs.
///
/// All methods take `&self` — implementations should use interior mutability
/// (e.g., append-mode file handles) when needed. Most read/write methods
/// take `(SessionId, SegmentId)` so segments can be physically grouped
/// per Session on disk (or per session_id in a DB).
pub trait Store: Send + Sync {
    /// Append a single log entry to the segment log.
    ///
    /// One line per call. The kernel orders concurrent `O_APPEND` writes
    /// for lines < `PIPE_BUF`, so user-space serialization is unnecessary.
    fn append(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        entry: &LogEntry,
    ) -> Result<(), StoreError>;

    /// Read all log entries for a segment, in order.
    fn read_all(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
    ) -> Result<Vec<LogEntry>, StoreError>;

    /// List all session IDs, most recent first.
    fn list_sessions(&self) -> Result<Vec<SessionId>, StoreError>;

    /// List segment IDs belonging to `session_id`, most recent first.
    fn list_segments(&self, session_id: SessionId) -> Result<Vec<SegmentId>, StoreError>;

    /// Look up which session a given segment belongs to. Returns `None`
    /// when the segment is not known to any session. Implementations
    /// may scan storage; intended for shim entry points that receive a
    /// segment ID without its session ID (e.g. legacy `--session <UUID>`).
    fn lookup_session_of(&self, segment_id: SegmentId) -> Result<Option<SessionId>, StoreError>;

    /// Create a new segment within `session_id`, with initial entries.
    fn create_segment(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        entries: &[LogEntry],
    ) -> Result<(), StoreError>;

    /// Check if a segment exists.
    fn exists(&self, session_id: SessionId, segment_id: SegmentId) -> Result<bool, StoreError>;

    /// Truncate a segment log to `entries_len` entries.
    ///
    /// Used by Pod's submit-time empty-turn rollback after it has proven
    /// that no LLM output from the accepted turn was materialized. The
    /// default implementation rewrites the retained prefix through
    /// `create_segment`, matching the append-only logical model while still
    /// allowing concrete stores to provide a more direct truncate.
    fn truncate(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        entries_len: usize,
    ) -> Result<(), StoreError> {
        let mut entries = self.read_all(session_id, segment_id)?;
        if entries_len > entries.len() {
            return Err(StoreError::Corrupt {
                line: entries_len,
                message: format!(
                    "cannot truncate segment {segment_id} to {entries_len} entries; only {} entries stored",
                    entries.len()
                ),
            });
        }
        entries.truncate(entries_len);
        self.create_segment(session_id, segment_id, &entries)
    }

    /// Count entries currently stored for a segment.
    ///
    /// Used by `ensure_head_or_fork` to detect concurrent writers:
    /// if the on-disk count exceeds the writer's own append tally,
    /// another process has extended the log.
    fn read_entry_count(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
    ) -> Result<usize, StoreError>;

    /// Append a trace entry to the debug event trace file.
    fn append_trace(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        entry: &TraceEntry,
    ) -> Result<(), StoreError>;
}
