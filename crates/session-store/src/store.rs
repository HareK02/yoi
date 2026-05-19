//! Persistence backend abstraction.
//!
//! [`Store`] defines the sync interface for reading and writing session logs.
//! Implementations handle the physical storage (filesystem, database, etc.).
//!
//! Sync (rather than async) is intentional: a session log append is a single
//! `< 1 KiB` line on local fs and completes well below a millisecond. Going
//! through `tokio::fs` would force every caller — including `Worker`'s sync
//! `on_history_append` callback — to bridge sync → async via a channel +
//! drain task. Keeping the store sync lets the worker callback, Pod commit
//! paths, and `PodInterceptor` all share one direct `append_entry` call.

use crate::SessionId;
use crate::event_trace::TraceEntry;
use crate::session_log::LogEntry;

/// Errors from the persistence store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("session not found: {0}")]
    NotFound(SessionId),

    #[error("log corrupted at line {line}: {message}")]
    Corrupt { line: usize, message: String },
}

/// Sync persistence backend for session logs.
///
/// All methods take `&self` — implementations should use interior mutability
/// (e.g., append-mode file handles) when needed.
pub trait Store: Send + Sync {
    /// Append a single log entry to the session log.
    ///
    /// One line per call. The kernel orders concurrent `O_APPEND` writes
    /// for lines < `PIPE_BUF`, so user-space serialization is unnecessary.
    fn append(&self, id: SessionId, entry: &LogEntry) -> Result<(), StoreError>;

    /// Read all log entries for a session, in order.
    fn read_all(&self, id: SessionId) -> Result<Vec<LogEntry>, StoreError>;

    /// List all session IDs, most recent first.
    fn list_sessions(&self) -> Result<Vec<SessionId>, StoreError>;

    /// Create a new session with initial entries.
    fn create_session(&self, id: SessionId, entries: &[LogEntry]) -> Result<(), StoreError>;

    /// Check if a session exists.
    fn exists(&self, id: SessionId) -> Result<bool, StoreError>;

    /// Count entries currently stored for a session.
    ///
    /// Used by `ensure_head_or_fork` to detect concurrent writers:
    /// if the on-disk count exceeds the writer's own append tally,
    /// another process has extended the log.
    fn read_entry_count(&self, id: SessionId) -> Result<usize, StoreError>;

    /// Append a trace entry to the debug event trace file.
    fn append_trace(&self, id: SessionId, entry: &TraceEntry) -> Result<(), StoreError>;
}
