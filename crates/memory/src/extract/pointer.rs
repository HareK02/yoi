//! Payload format for `LogEntry::Extension { domain: "memory.extract", payload }`
//! and fold helper for restore. The memory crate owns the domain, so
//! session-store / Worker do not know the payload structure.

use serde::{Deserialize, Serialize};

use super::EXTRACT_DOMAIN;

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

/// Returns the latest extraction pointer, or `None` if absent or the latest payload is invalid.
/// No fallback: ignoring a corrupt latest entry and using an older pointer would yield the wrong re-extraction range.
pub fn fold_pointer(extensions: &[(String, serde_json::Value)]) -> Option<ExtractPointerPayload> {
    extensions
        .iter()
        .rev()
        .find(|(domain, _)| domain == EXTRACT_DOMAIN)
        .and_then(|(_, value)| serde_json::from_value(value.clone()).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_returns_latest_when_multiple_present() {
        let exts = vec![
            (
                EXTRACT_DOMAIN.to_string(),
                serde_json::json!({
                    "processed_through_entry": 5,
                    "processed_through_history_len": 4,
                    "staging_id": "old"
                }),
            ),
            ("other.domain".to_string(), serde_json::json!({ "x": 1 })),
            (
                EXTRACT_DOMAIN.to_string(),
                serde_json::json!({
                    "processed_through_entry": 11,
                    "processed_through_history_len": 8,
                    "staging_id": "new"
                }),
            ),
        ];
        let p = fold_pointer(&exts).unwrap();
        assert_eq!(p.processed_through_entry, 11);
        assert_eq!(p.processed_through_history_len, 8);
        assert_eq!(p.staging_id, "new");
    }

    #[test]
    fn fold_returns_none_when_absent() {
        let exts = vec![("other.domain".to_string(), serde_json::json!({ "x": 1 }))];
        assert!(fold_pointer(&exts).is_none());
    }

    #[test]
    fn fold_skips_malformed_entries() {
        let exts = vec![(
            EXTRACT_DOMAIN.to_string(),
            serde_json::json!({ "wrong_shape": true }),
        )];
        // Takes the latest entry and returns None if its JSON does not match.
        // No fallback: silently ignoring a corrupt latest entry could cause unintended re-extraction.
        assert!(fold_pointer(&exts).is_none());
    }
}
