//! Session persistence via append-only JSONL logs.
//!
//! # Architecture
//!
//! Sessions are recorded as a sequence of [`LogEntry`] values, one per line
//! in a `.jsonl` file. Reading the log and collecting entries reconstructs
//! the full Worker state — no separate snapshots or checkpoints needed.
//!
//! This crate provides free functions for persistence operations.
//! The caller (typically Pod) holds the Worker directly and calls these
//! functions after state-mutating operations.
//!
//! Debug-mode [`TraceEntry`] records capture raw stream events in a separate
//! `.trace.jsonl` file, independent of the session log.
//!
//! # Quick start
//!
//! ```ignore
//! use session_store::{create_session, restore, save_delta, FsStore, SessionStartState};
//!
//! let store = FsStore::new("./sessions").await?;
//! let (session_id, head_hash) = create_session(&store, SessionStartState {
//!     system_prompt: None,
//!     config: &config,
//!     history: &[],
//! }).await?;
//! ```

pub mod event_trace;
pub mod fs_store;
pub mod session;
pub mod session_log;
pub mod store;

pub use event_trace::TraceEntry;
pub use fs_store::FsStore;
pub use session::{
    SessionStartState, create_compacted_session, create_session, create_session_with_id,
    ensure_head_or_fork, fork, fork_at, restore, save_config_changed, save_delta, save_extension,
    save_run_completed, save_run_errored, save_turn_end, save_usage,
};
pub use llm_worker::UsageRecord;
pub use llm_worker::llm_client::types::{ContentPart, Item, Role};
pub use session_log::{
    EntryHash, HashedEntry, LogEntry, RestoredState, SessionOrigin, build_chain, collect_state,
    compute_hash,
};
pub use store::{Store, StoreError};

/// Session identifier. UUID v7 (time-ordered, lexicographically sortable).
pub type SessionId = uuid::Uuid;

/// Generate a new session ID.
pub fn new_session_id() -> SessionId {
    uuid::Uuid::now_v7()
}
