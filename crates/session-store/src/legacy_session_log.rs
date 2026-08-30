//! Versioned decoder for Session schemas that predate canonical annotated history.
//!
//! These types are intentionally private to `session-store`. Current writers,
//! replay, and public projections use [`crate::LogEntry`] exclusively; only the
//! Worker Session schema migration is allowed to deserialize these shapes.

use agen::llm_client::types::RequestConfig;
use protocol::Segment;
use serde::Deserialize;

use crate::{
    LogEntry, LoggedHistoryEntry, LoggedItem, LoggedSessionHistoryEntryId,
    LoggedSessionHistoryMetadata, LoggedSessionHistoryOrigin, LoggedSystemHistoryEntry, SegmentId,
    SegmentOrigin, SessionExtension, SessionId, SystemItem,
};

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum LegacyHistoryLogEntry {
    SegmentStart {
        ts: u64,
        session_id: SessionId,
        system_prompt: Option<String>,
        config: RequestConfig,
        history: Vec<LoggedItem>,
        #[serde(default)]
        forked_from: Option<SegmentOrigin>,
        #[serde(default)]
        compacted_from: Option<SegmentOrigin>,
    },
    UserInput {
        ts: u64,
        segments: Vec<Segment>,
        #[serde(default)]
        extensions: Vec<SessionExtension>,
    },
    AssistantItem {
        ts: u64,
        item: LoggedItem,
    },
    ToolResult {
        ts: u64,
        item: LoggedItem,
    },
    SystemItem {
        ts: u64,
        item: SystemItem,
    },
}

/// Schema-v1 decoder. Non-history records already had their current shape, so
/// they pass through `LogEntry`; legacy history records are converted below.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum LegacySessionLogEntryV1 {
    History(LegacyHistoryLogEntry),
    Current(LogEntry),
}

/// Schema v2 retained the v1 history shapes while adding non-history records.
/// Keep a distinct type so supported source versions remain explicit rather
/// than turning migration compatibility into the current `LogEntry` contract.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum LegacySessionLogEntryV2 {
    History(LegacyHistoryLogEntry),
    Current(LogEntry),
}

pub(crate) fn decode_entry(
    schema_version: u32,
    line: &str,
    session_id: SessionId,
    segment_id: SegmentId,
    line_index: usize,
) -> Result<LogEntry, serde_json::Error> {
    let entry = match schema_version {
        1 => match serde_json::from_str::<LegacySessionLogEntryV1>(line)? {
            LegacySessionLogEntryV1::History(entry) => Entry::History(entry),
            LegacySessionLogEntryV1::Current(entry) => Entry::Current(entry),
        },
        2 => match serde_json::from_str::<LegacySessionLogEntryV2>(line)? {
            LegacySessionLogEntryV2::History(entry) => Entry::History(entry),
            LegacySessionLogEntryV2::Current(entry) => Entry::Current(entry),
        },
        _ => unreachable!("legacy decoder called for unsupported schema {schema_version}"),
    };
    Ok(match entry {
        Entry::History(entry) => {
            canonicalize_history_entry(session_id, segment_id, line_index, entry)
        }
        Entry::Current(entry) => entry,
    })
}

enum Entry {
    History(LegacyHistoryLogEntry),
    Current(LogEntry),
}

fn legacy_metadata(
    segment_id: SegmentId,
    line_index: usize,
    item_index: usize,
) -> LoggedSessionHistoryMetadata {
    let mut identity = Vec::with_capacity(32);
    identity.extend_from_slice(segment_id.as_bytes());
    identity.extend_from_slice(&(line_index as u64).to_be_bytes());
    identity.extend_from_slice(&(item_index as u64).to_be_bytes());
    LoggedSessionHistoryMetadata {
        entry_id: LoggedSessionHistoryEntryId(format!(
            "l-{}",
            base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, identity)
        )),
        origin: LoggedSessionHistoryOrigin::LegacyUnknown,
        derivation: None,
    }
}

fn canonicalize_history_entry(
    _session_id: SessionId,
    segment_id: SegmentId,
    line_index: usize,
    entry: LegacyHistoryLogEntry,
) -> LogEntry {
    match entry {
        LegacyHistoryLogEntry::SegmentStart {
            ts,
            session_id,
            system_prompt,
            config,
            history,
            forked_from,
            compacted_from,
        } => LogEntry::AnnotatedSegmentStart {
            ts,
            session_id,
            system_prompt,
            config,
            history: history
                .into_iter()
                .enumerate()
                .map(|(item_index, item)| LoggedHistoryEntry {
                    item,
                    metadata: legacy_metadata(segment_id, line_index, item_index),
                })
                .collect(),
            forked_from,
            compacted_from,
        },
        LegacyHistoryLogEntry::UserInput {
            ts,
            segments,
            extensions,
        } => LogEntry::AnnotatedUserInput {
            ts,
            history: vec![LoggedHistoryEntry {
                item: LoggedItem::from(agen::Item::user_message(Segment::flatten_to_text(
                    &segments,
                ))),
                metadata: legacy_metadata(segment_id, line_index, 0),
            }],
            segments,
            extensions,
        },
        LegacyHistoryLogEntry::AssistantItem { ts, item } => LogEntry::AnnotatedAssistantItem {
            ts,
            entry: LoggedHistoryEntry {
                item,
                metadata: legacy_metadata(segment_id, line_index, 0),
            },
        },
        LegacyHistoryLogEntry::ToolResult { ts, item } => LogEntry::AnnotatedToolResult {
            ts,
            entry: LoggedHistoryEntry {
                item,
                metadata: legacy_metadata(segment_id, line_index, 0),
            },
        },
        LegacyHistoryLogEntry::SystemItem { ts, item } => LogEntry::AnnotatedSystemItem {
            ts,
            entry: LoggedSystemHistoryEntry {
                item,
                metadata: legacy_metadata(segment_id, line_index, 0),
            },
        },
    }
}
