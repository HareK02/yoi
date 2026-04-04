//! Session persistence for `llm-worker` via append-only JSONL logs.
//!
//! # Architecture
//!
//! Sessions are recorded as a sequence of [`LogEntry`] values, one per line
//! in a `.jsonl` file. Reading the log and collecting entries reconstructs
//! the full [`Worker`] state — no separate snapshots or checkpoints needed.
//!
//! Debug-mode [`TraceEntry`] records capture raw stream events in a separate
//! `.trace.jsonl` file, independent of the session log.
//!
//! # Quick start
//!
//! ```ignore
//! use llm_worker_persistence::{Session, SessionConfig, FsStore};
//!
//! let store = FsStore::new("./sessions").await?;
//! let worker = Worker::new(client);
//! let mut session = Session::new(worker, store, SessionConfig::default()).await?;
//! session.run("Hello!").await?;
//! ```

pub mod event_trace;
pub mod fs_store;
pub mod session;
pub mod session_log;
pub mod store;

pub use event_trace::TraceEntry;
pub use fs_store::FsStore;
pub use session::{Session, SessionConfig, SessionError};
pub use session_log::{LogEntry, Outcome, RestoredState, collect_state};
pub use store::{Store, StoreError};

/// Session identifier. UUID v7 (time-ordered, lexicographically sortable).
pub type SessionId = uuid::Uuid;

/// Generate a new session ID.
pub fn new_session_id() -> SessionId {
    uuid::Uuid::now_v7()
}
