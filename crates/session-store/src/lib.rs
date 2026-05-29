//! Session persistence via append-only JSONL logs.
//!
//! # Architecture
//!
//! A [`Session`](SessionId) is a fork-tree of [`Segment`](SegmentId)s
//! belonging to the same logical conversation. Each Segment is recorded
//! as a sequence of [`LogEntry`] values, one per line in a `.jsonl`
//! file. Reading a segment log and collecting entries reconstructs the
//! Worker state at that segment — no separate snapshots or checkpoints
//! needed. Compaction and fork operations mint a fresh Segment within
//! the same Session.
//!
//! This crate provides free functions for persistence operations.
//! The caller (typically Pod) holds the Worker directly and calls these
//! functions after state-mutating operations.
//!
//! Debug-mode [`TraceEntry`] records capture raw stream events in a separate
//! `.trace.jsonl` file, independent of the segment log.
//!
//! # Quick start
//!
//! ```ignore
//! use session_store::{create_segment, restore, save_delta, FsStore, SegmentStartState};
//!
//! let store = FsStore::new("./sessions")?;
//! let (session_id, segment_id) = create_segment(&store, SegmentStartState {
//!     system_prompt: None,
//!     config: &config,
//!     history: &[],
//! })?;
//! ```

pub mod event_trace;
pub mod fs_store;
pub mod logged_item;
pub mod pod_metadata;
pub mod segment;
pub mod segment_log;
pub mod store;
pub mod system_item;

pub use event_trace::{TraceEntry, TracePayload};
pub use fs_store::FsStore;
pub use llm_worker::UsageRecord;
pub use llm_worker::llm_client::types::{ContentPart, Item, Role};
pub use logged_item::{LoggedContentPart, LoggedItem, LoggedRole, from_logged, to_logged};
pub use pod_metadata::{
    PodActiveSegmentRef, PodMetadata, PodMetadataStore, PodSpawnedChild, PodSpawnedScopeRule,
};
pub use segment::{
    SegmentStartState, append_entry, append_system_item, classify_history_item,
    create_compacted_segment, create_segment, create_segment_with_ids, ensure_head_or_fork, fork,
    fork_at, restore, restore_by_segment, save_config_changed, save_delta, save_extension,
    save_pod_scope, save_run_completed, save_run_errored, save_turn_end, save_usage,
    save_user_input,
};
pub use segment_log::{
    LogEntry, POD_SCOPE_EXTENSION_DOMAIN, PodScopeSnapshot, RestoredState, SegmentOrigin,
    collect_state,
};
pub use store::{Store, StoreError};
pub use system_item::{SystemItem, SystemReminder, SystemReminderSource, render_pod_event};

/// Session identifier — the fork-tree root. UUID v7 (time-ordered).
///
/// All Segments belonging to the same Session share this ID. Compaction
/// and fork operations create a new Segment within the same Session, so
/// `WHERE session_id = ?` retrieves the full lineage.
pub type SessionId = uuid::Uuid;

/// Segment identifier. UUID v7 (time-ordered, lexicographically sortable).
pub type SegmentId = uuid::Uuid;

/// Generate a new session ID.
pub fn new_session_id() -> SessionId {
    uuid::Uuid::now_v7()
}

/// Generate a new segment ID.
pub fn new_segment_id() -> SegmentId {
    uuid::Uuid::now_v7()
}
