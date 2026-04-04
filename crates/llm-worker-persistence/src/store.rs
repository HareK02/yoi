//! Persistence backend abstraction.
//!
//! [`Store`] defines the async interface for reading and writing session logs.
//! Implementations handle the physical storage (filesystem, database, etc.).

use crate::event_trace::TraceEntry;
use crate::session_log::LogEntry;
use crate::SessionId;
use std::future::Future;

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

/// Async persistence backend for session logs.
///
/// All methods take `&self` — implementations should use interior mutability
/// (e.g., append-mode file handles) when needed.
pub trait Store: Send + Sync {
    /// Append a single log entry to the session.
    fn append(
        &self,
        id: SessionId,
        entry: &LogEntry,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Read all log entries for a session, in order.
    fn read_all(
        &self,
        id: SessionId,
    ) -> impl Future<Output = Result<Vec<LogEntry>, StoreError>> + Send;

    /// List all session IDs, most recent first.
    fn list_sessions(&self)
        -> impl Future<Output = Result<Vec<SessionId>, StoreError>> + Send;

    /// Create a new session with initial entries.
    fn create_session(
        &self,
        id: SessionId,
        entries: &[LogEntry],
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Check if a session exists.
    fn exists(
        &self,
        id: SessionId,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Append a trace entry to the debug event trace file.
    fn append_trace(
        &self,
        id: SessionId,
        entry: &TraceEntry,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
}
