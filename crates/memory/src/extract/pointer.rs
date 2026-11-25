//! Progress payload used by the Host's Subject extraction lifecycle.
//!
//! The Host owns the Session extension domain and validates the latest pointer
//! against committed history. This payload does not select a storage destination.

use serde::{Deserialize, Serialize};

/// Payload recording an extraction completion boundary as a Session Extension entry.
/// Only the latest entry is the current pointer; the Host lifecycle checks storage completion conditions.
/// This commits separately from candidate storage, so saved candidates may remain even without a committed pointer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtractPointerPayload {
    /// Index of the last session-store LogEntry processed by the latest extraction.
    /// The next `source.range.start` is this value + 1.
    pub processed_through_entry: usize,
    /// `history.len()` at the latest extraction. The next input slices
    /// `history[processed_through_history_len..]`.
    pub processed_through_history_len: usize,
    /// Staging record ID returned by storage. Completion without candidates may use an empty value.
    /// Does not denote a local file's existence or storage path.
    pub staging_id: String,
}
