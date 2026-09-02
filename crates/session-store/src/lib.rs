//! Session persistence via append-only JSONL logs.
//!
//! # Architecture
//!
//! A [`Session`](SessionId) is a fork-tree of [`Segment`](SegmentId)s
//! belonging to the same logical conversation. Each Segment is recorded
//! as a sequence of [`LogEntry`] values, one per line in a `.jsonl`
//! file. Reading a segment log and collecting entries reconstructs the
//! Engine state at that segment — no separate snapshots or checkpoints
//! needed. Compaction and fork operations mint a fresh Segment within
//! the same Session.
//!
//! This crate provides free functions for persistence operations.
//! The caller (typically Worker) holds the Engine directly and calls these
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
//!     history: Vec::new(),
//! })?;
//! ```

pub mod event_trace;
pub mod fs_store;
pub mod history;
mod legacy_session_log;
pub mod logged_item;
mod paste_artifact;
pub mod public_snapshot;
pub mod segment;
pub mod segment_log;
pub mod store;
pub mod system_item;
pub mod uploaded_file;
pub mod worker_metadata;
pub mod worker_session_store;

pub use agen::UsageRecord;
pub use agen::llm_client::types::{ContentPart, Item, Role};
pub use event_trace::{TraceEntry, TracePayload};
pub use fs_store::FsStore;
pub use history::{
    LoggedHistoryDerivation, LoggedHistoryEntry, LoggedSessionHistoryEntryId,
    LoggedSessionHistoryMetadata, LoggedSessionHistoryOrigin, LoggedSystemHistoryEntry,
    LoggedWorkerSubject,
};
pub use logged_item::{LoggedContentPart, LoggedItem, LoggedRole, from_logged, to_logged};
pub use paste_artifact::PasteArtifactLimits;
pub use segment::{
    SegmentStartState, append_entry, append_system_item, classify_logged_history_entry,
    create_compacted_segment, create_segment, create_segment_with_ids, ensure_head_or_fork, fork,
    fork_at, restore, restore_by_segment, save_config_changed, save_delta, save_extension,
    save_run_completed, save_run_errored, save_turn_end, save_usage, save_user_input,
};
pub use segment_log::{LogEntry, RestoredState, SegmentOrigin, SessionExtension, collect_state};
pub use store::{Store, StoreError};
pub use system_item::{
    PromptRenderProvenance, SystemItem, SystemReminder, SystemReminderSource, render_worker_event,
};
pub use uploaded_file::{
    DEFAULT_MAX_FILES_PER_SUBMISSION, DEFAULT_MAX_SESSION_ARTIFACT_BYTES,
    DEFAULT_MAX_SESSION_UPLOADED_FILES, DEFAULT_MAX_UPLOADED_FILE_BYTES, UploadedFileLimits,
};
pub use worker_metadata::{
    CombinedStore, FsWorkerStore, WorkerActiveSegmentRef, WorkerAggregateStore, WorkerMetadata,
    WorkerMetadataStore, WorkerPeer, WorkerReclaimedChild, WorkerSpawnedChild,
    WorkerSpawnedScopeRule, WorkerStoreError, validate_worker_name,
};
pub use worker_session_store::WorkerSessionStore;

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
