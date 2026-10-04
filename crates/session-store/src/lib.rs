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
//!     user_segments: Vec::new(),
//! })?;
//! ```

use std::io;
use std::path::Path;

/// Read an existing retained file without updating its access timestamp.
/// Observation fails closed when the platform cannot provide that guarantee.
pub(crate) fn read_without_atime(path: &Path) -> io::Result<Vec<u8>> {
    read_without_atime_bounded(path, u64::MAX)
}

/// Read at most `max_bytes + 1` retained bytes without updating access time.
/// The extra byte lets callers prove that the authoritative file exceeded a
/// limit without first racing a separate metadata length observation.
pub(crate) fn read_without_atime_bounded(path: &Path, max_bytes: u64) -> io::Result<Vec<u8>> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use std::fs::OpenOptions;
        use std::io::Read;
        use std::os::unix::fs::OpenOptionsExt;

        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOATIME)
            .open(path)?;
        let mut bytes = Vec::new();
        file.take(max_bytes.saturating_add(1))
            .read_to_end(&mut bytes)?;
        Ok(bytes)
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let _ = (path, max_bytes);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "read-only retained observation requires no-atime file reads",
        ))
    }
}

pub mod event_trace;
pub mod fs_store;
pub mod history;
mod legacy_session_log;
pub mod logged_item;
mod paste_artifact;
pub mod public_index;
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
pub use public_index::{
    DEFAULT_SESSION_PUBLIC_INDEX_MAX_BYTES, DEFAULT_SESSION_PUBLIC_INDEX_MAX_ENTRIES,
    DEFAULT_SESSION_PUBLIC_INDEX_MAX_SEGMENTS, SESSION_PUBLIC_INDEX_COMPACT_TEXT_MAX_BYTES,
    SessionPublicIndex, SessionPublicIndexEntry, SessionPublicIndexEntryKind,
    SessionPublicIndexEntryRead, SessionPublicIndexLimits, SessionPublicIndexLineage,
    SessionPublicIndexOriginKind, SessionPublicIndexPage, SessionPublicIndexPageEntry,
    SessionPublicIndexReadError, SessionPublicIndexScanPosition, SessionPublicIndexSegment,
    SessionPublicIndexToolPart, read_session_public_index, read_session_public_index_entry,
    read_session_public_index_page,
};
pub use public_snapshot::{
    DEFAULT_RETAINED_HISTORY_MAX_ENTRIES, DEFAULT_RETAINED_HISTORY_MAX_RESPONSE_BYTES,
    DEFAULT_RETAINED_HISTORY_MAX_SCAN_BYTES, DEFAULT_RETAINED_HISTORY_MAX_SEGMENTS,
    DEFAULT_RETAINED_HISTORY_PAGE_TURNS, DEFAULT_RETAINED_SNAPSHOT_MAX_BYTES,
    MAX_RETAINED_HISTORY_PAGE_TURNS, RetainedAttachmentReadError, RetainedHistoryReadError,
    RetainedHistoryReadLimits, RetainedSessionAttachment, RetainedSessionIdentity,
    RetainedSessionSnapshot, RetainedSnapshotReadError, project_session_snapshot,
    read_retained_session_attachment, read_retained_session_history_page,
    read_retained_session_snapshot, read_session_attachment_from_entries,
    session_tool_attachment_id,
};
pub use segment::{
    SegmentStartState, append_entry, append_system_item, classify_logged_history_entry,
    create_compacted_segment, create_segment, create_segment_with_ids, ensure_head_or_fork, fork,
    fork_at, restore, restore_by_segment, save_config_changed, save_delta, save_extension,
    save_run_completed, save_run_errored, save_run_resumed, save_run_yielded, save_turn_end,
    save_usage, save_user_input,
};
pub use segment_log::{LogEntry, RestoredState, SegmentOrigin, SessionExtension, collect_state};
pub use store::{Store, StoreError};
pub use system_item::{
    PromptRenderProvenance, SystemItem, SystemReminder, SystemReminderSource, render_worker_event,
};
pub use uploaded_file::{
    DEFAULT_MAX_FILES_PER_SUBMISSION, DEFAULT_MAX_SESSION_ARTIFACT_BYTES,
    DEFAULT_MAX_SESSION_UPLOADED_FILES, DEFAULT_MAX_UPLOADED_FILE_BYTES, UploadedFileLimits,
    UploadedFileUploadContext,
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
