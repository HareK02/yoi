use std::collections::{HashMap, HashSet};
use std::path::Path;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use protocol::{
    Segment, SessionContentPart, SessionEntryProvenance, SessionMessageRole, SessionSnapshotEntry,
    SessionSnapshotEntryData,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{LogEntry, SegmentId, SegmentOrigin, SessionId, StoreError, WorkerSessionStore};

pub const DEFAULT_SESSION_PUBLIC_INDEX_MAX_BYTES: u64 = 64 * 1024 * 1024;
pub const DEFAULT_SESSION_PUBLIC_INDEX_MAX_SEGMENTS: usize = 64;
pub const DEFAULT_SESSION_PUBLIC_INDEX_MAX_ENTRIES: usize = 100_000;
pub const SESSION_PUBLIC_INDEX_COMPACT_TEXT_MAX_BYTES: usize = 512;

/// Caller-owned resource bounds for one complete public Session index read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionPublicIndexLimits {
    pub max_bytes: u64,
    pub max_segments: usize,
    pub max_entries: usize,
}

impl Default for SessionPublicIndexLimits {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_SESSION_PUBLIC_INDEX_MAX_BYTES,
            max_segments: DEFAULT_SESSION_PUBLIC_INDEX_MAX_SEGMENTS,
            max_entries: DEFAULT_SESSION_PUBLIC_INDEX_MAX_ENTRIES,
        }
    }
}

/// Read failures are intentionally storage-oriented. Authorization and archive
/// availability remain the responsibility of the caller that selected this
/// canonical Session root.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum SessionPublicIndexReadError {
    #[error("Worker Session requires migration")]
    MigrationRequired,
    #[error("Worker Session is missing")]
    Missing,
    #[error("Worker Session is corrupt")]
    Corrupt,
    #[error("Worker Session storage is unavailable")]
    Storage,
    #[error("Worker Session public index exceeds a resource limit")]
    ResourceLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPublicIndex {
    pub session_id: String,
    /// Content generation over the canonical Session identity, ordered Segment
    /// identities, lineage, and committed log records.
    pub generation: String,
    /// Every persisted Segment, including empty-public and non-active branches,
    /// in ascending `segment_id` order.
    pub segments: Vec<SessionPublicIndexSegment>,
    /// Persisted bytes actually consumed while constructing this generation.
    pub scanned_bytes: u64,
    /// Persisted records plus embedded history entries charged to the scan.
    pub scanned_entries: usize,
    pub entry_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPublicIndexSegment {
    pub segment_id: String,
    pub lineage: SessionPublicIndexLineage,
    pub entries: Vec<SessionPublicIndexEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPublicIndexOriginKind {
    Root,
    Fork,
    Compact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPublicIndexLineage {
    pub origin_kind: SessionPublicIndexOriginKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_segment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_turn_index: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPublicIndexEntryKind {
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPublicIndexToolPart {
    Input,
    Output,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPublicIndexEntry {
    pub segment_id: String,
    /// Stable public reference. The same inherited logical entry intentionally
    /// has the same ref in multiple Segments; `segment_id` disambiguates it.
    pub entry_ref: String,
    pub kind: SessionPublicIndexEntryKind,
    pub provenance: SessionEntryProvenance,
    pub timestamp: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_part: Option<SessionPublicIndexToolPart>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    pub compact_text: String,
    pub full_text: String,
}

#[derive(Debug)]
struct LoadedSegment {
    segment_id: SegmentId,
    lineage: SessionPublicIndexLineage,
    origin: Option<SegmentOrigin>,
    entries: Vec<LogEntry>,
    max_turn_index: usize,
}

/// Read every persisted Segment below one canonical `<worker>/session` root.
///
/// This operation is strictly observational: it opens the current schema in
/// read-only mode, performs no migration or restore, ignores trace and artifact
/// files, and reads Segment logs without intentionally updating access time.
pub fn read_session_public_index(
    session_root: &Path,
    limits: SessionPublicIndexLimits,
) -> Result<SessionPublicIndex, SessionPublicIndexReadError> {
    if limits.max_bytes == 0 || limits.max_segments == 0 || limits.max_entries == 0 {
        return Err(SessionPublicIndexReadError::ResourceLimit);
    }

    let store = WorkerSessionStore::open_read_only(session_root).map_err(map_store_open_error)?;
    let session_id = store
        .session_id()
        .map_err(map_store_error)?
        .ok_or(SessionPublicIndexReadError::Missing)?;
    let segment_ids = store
        .list_segments_read_only(session_id)
        .map_err(map_store_error)?;
    if segment_ids.len() > limits.max_segments {
        return Err(SessionPublicIndexReadError::ResourceLimit);
    }

    let mut scanned_bytes = 0_u64;
    let mut scanned_entries = 0_usize;
    let mut loaded = Vec::with_capacity(segment_ids.len());
    let mut generation = Sha256::new();
    generation.update(b"yoi-session-public-index-v1\0");
    generation.update(session_id.as_bytes());

    for segment_id in segment_ids {
        let remaining = limits
            .max_bytes
            .checked_sub(scanned_bytes)
            .ok_or(SessionPublicIndexReadError::ResourceLimit)?;
        let (entries, bytes) = store
            .read_all_read_only_bounded(session_id, segment_id, remaining)
            .map_err(map_store_error)?;
        scanned_bytes = scanned_bytes
            .checked_add(bytes)
            .ok_or(SessionPublicIndexReadError::ResourceLimit)?;
        let units = persisted_entry_units(&entries);
        scanned_entries = scanned_entries
            .checked_add(units)
            .ok_or(SessionPublicIndexReadError::ResourceLimit)?;
        if scanned_entries > limits.max_entries {
            return Err(SessionPublicIndexReadError::ResourceLimit);
        }

        let (lineage, origin) = validate_segment_start(session_id, segment_id, &entries)?;
        let max_turn_index = entries.iter().fold(
            origin
                .as_ref()
                .map(|origin| origin.at_turn_index)
                .unwrap_or(0),
            |max_turn, entry| match entry {
                LogEntry::TurnEnd { turn_count, .. } => max_turn.max(*turn_count),
                _ => max_turn,
            },
        );

        generation.update(segment_id.as_bytes());
        generation.update((entries.len() as u64).to_be_bytes());
        for entry in &entries {
            let encoded =
                serde_json::to_vec(entry).map_err(|_| SessionPublicIndexReadError::Corrupt)?;
            generation.update((encoded.len() as u64).to_be_bytes());
            generation.update(encoded);
        }

        loaded.push(LoadedSegment {
            segment_id,
            lineage,
            origin,
            entries,
            max_turn_index,
        });
    }

    validate_lineage(&loaded)?;

    let mut entry_count = 0_usize;
    let mut segments = Vec::with_capacity(loaded.len());
    for segment in loaded {
        let committed = committed_conversation_records(&segment.entries);
        let snapshot = crate::public_snapshot::project_session_snapshot_for_segment(
            session_id,
            Some(segment.segment_id),
            &committed,
        );
        let entries = project_index_entries(segment.segment_id, snapshot.entries)?;
        entry_count = entry_count
            .checked_add(entries.len())
            .ok_or(SessionPublicIndexReadError::ResourceLimit)?;
        segments.push(SessionPublicIndexSegment {
            segment_id: segment.segment_id.to_string(),
            lineage: segment.lineage,
            entries,
        });
    }

    Ok(SessionPublicIndex {
        session_id: session_id.to_string(),
        generation: URL_SAFE_NO_PAD.encode(generation.finalize()),
        segments,
        scanned_bytes,
        scanned_entries,
        entry_count,
    })
}

fn validate_segment_start(
    expected_session_id: SessionId,
    segment_id: SegmentId,
    entries: &[LogEntry],
) -> Result<(SessionPublicIndexLineage, Option<SegmentOrigin>), SessionPublicIndexReadError> {
    let Some(LogEntry::AnnotatedSegmentStart {
        session_id,
        forked_from,
        compacted_from,
        ..
    }) = entries.first()
    else {
        return Err(SessionPublicIndexReadError::Corrupt);
    };
    if *session_id != expected_session_id
        || entries
            .iter()
            .skip(1)
            .any(|entry| matches!(entry, LogEntry::AnnotatedSegmentStart { .. }))
    {
        return Err(SessionPublicIndexReadError::Corrupt);
    }

    match (forked_from, compacted_from) {
        (Some(_), Some(_)) => Err(SessionPublicIndexReadError::Corrupt),
        (Some(origin), None) => validate_origin(segment_id, origin).map(|origin| {
            (
                SessionPublicIndexLineage {
                    origin_kind: SessionPublicIndexOriginKind::Fork,
                    parent_segment_id: Some(origin.segment_id.to_string()),
                    parent_turn_index: Some(origin.at_turn_index),
                },
                Some(origin),
            )
        }),
        (None, Some(origin)) => validate_origin(segment_id, origin).map(|origin| {
            (
                SessionPublicIndexLineage {
                    origin_kind: SessionPublicIndexOriginKind::Compact,
                    parent_segment_id: Some(origin.segment_id.to_string()),
                    parent_turn_index: Some(origin.at_turn_index),
                },
                Some(origin),
            )
        }),
        (None, None) => Ok((
            SessionPublicIndexLineage {
                origin_kind: SessionPublicIndexOriginKind::Root,
                parent_segment_id: None,
                parent_turn_index: None,
            },
            None,
        )),
    }
}

fn validate_origin(
    segment_id: SegmentId,
    origin: &SegmentOrigin,
) -> Result<SegmentOrigin, SessionPublicIndexReadError> {
    if origin.segment_id == segment_id {
        return Err(SessionPublicIndexReadError::Corrupt);
    }
    Ok(origin.clone())
}

fn validate_lineage(segments: &[LoadedSegment]) -> Result<(), SessionPublicIndexReadError> {
    let by_id: HashMap<SegmentId, &LoadedSegment> = segments
        .iter()
        .map(|segment| (segment.segment_id, segment))
        .collect();
    for segment in segments {
        if let Some(origin) = &segment.origin {
            let parent = by_id
                .get(&origin.segment_id)
                .ok_or(SessionPublicIndexReadError::Corrupt)?;
            if origin.at_turn_index > parent.max_turn_index {
                return Err(SessionPublicIndexReadError::Corrupt);
            }
        }

        let mut seen = HashSet::new();
        let mut current = segment;
        while let Some(origin) = &current.origin {
            if !seen.insert(current.segment_id) {
                return Err(SessionPublicIndexReadError::Corrupt);
            }
            current = by_id
                .get(&origin.segment_id)
                .copied()
                .ok_or(SessionPublicIndexReadError::Corrupt)?;
        }
    }
    Ok(())
}

/// Project only conversation records whose run reached a durable public commit
/// boundary. Human input written before `Invoke` remains committed input, while
/// assistant/tool output accumulated by a cancelled, errored, abandoned, or
/// still-active run is discarded. A yielded run is a committed boundary and a
/// later `RunResumed` begins a new pending buffer.
fn committed_conversation_records(entries: &[LogEntry]) -> Vec<LogEntry> {
    let mut committed = Vec::with_capacity(entries.len());
    let mut pending_run: Option<Vec<LogEntry>> = None;

    for entry in entries {
        match entry {
            LogEntry::Invoke { .. } | LogEntry::RunResumed { .. } => {
                pending_run = Some(vec![entry.clone()]);
            }
            LogEntry::RunCompleted { .. } | LogEntry::RunYielded { .. } => {
                if let Some(mut pending) = pending_run.take() {
                    pending.push(entry.clone());
                    committed.append(&mut pending);
                } else {
                    committed.push(entry.clone());
                }
            }
            LogEntry::RunCancelled { .. }
            | LogEntry::RunErrored { .. }
            | LogEntry::PausedTurnAbandoned { .. } => {
                pending_run = None;
            }
            _ => {
                if let Some(pending) = &mut pending_run {
                    pending.push(entry.clone());
                } else {
                    committed.push(entry.clone());
                }
            }
        }
    }

    committed
}

fn project_index_entries(
    segment_id: SegmentId,
    entries: Vec<SessionSnapshotEntry>,
) -> Result<Vec<SessionPublicIndexEntry>, SessionPublicIndexReadError> {
    let mut projected = Vec::new();
    let mut seen_refs = HashSet::new();
    let mut tool_names = HashMap::<String, String>::new();

    for entry in entries {
        if entry.entry_id.is_empty() {
            return Err(SessionPublicIndexReadError::Corrupt);
        }
        let entry_ref = format!("E{}", entry.entry_id);
        if !seen_refs.insert(entry_ref.clone()) {
            continue;
        }

        let (kind, tool_part, tool_name, compact_source, full_text) = match entry.data {
            SessionSnapshotEntryData::UserInput { segments } => {
                let text = Segment::flatten_to_text(&segments);
                (
                    SessionPublicIndexEntryKind::User,
                    None,
                    None,
                    text.clone(),
                    text,
                )
            }
            SessionSnapshotEntryData::Message { role, content } => {
                let text = content_parts_text(&content);
                let kind = match role {
                    SessionMessageRole::User => SessionPublicIndexEntryKind::User,
                    SessionMessageRole::Assistant => SessionPublicIndexEntryKind::Assistant,
                };
                (kind, None, None, text.clone(), text)
            }
            SessionSnapshotEntryData::ToolCall {
                call_id,
                name,
                arguments,
            } => {
                tool_names.insert(call_id, name.clone());
                (
                    SessionPublicIndexEntryKind::Tool,
                    Some(SessionPublicIndexToolPart::Input),
                    Some(name),
                    arguments.clone(),
                    arguments,
                )
            }
            SessionSnapshotEntryData::ToolResult {
                call_id,
                summary,
                content,
                ..
            } => {
                let full = content.unwrap_or_else(|| summary.clone());
                let compact = if summary.is_empty() {
                    full.clone()
                } else {
                    summary
                };
                (
                    SessionPublicIndexEntryKind::Tool,
                    Some(SessionPublicIndexToolPart::Output),
                    tool_names.get(&call_id).cloned(),
                    compact,
                    full,
                )
            }
            // SystemItems, run transitions, and any future non-conversation
            // public snapshot records do not cross this narrower index boundary.
            _ => continue,
        };

        projected.push(SessionPublicIndexEntry {
            segment_id: segment_id.to_string(),
            entry_ref,
            kind,
            provenance: entry.provenance,
            timestamp: entry.timestamp,
            tool_part,
            tool_name,
            compact_text: truncate_utf8(
                &compact_source,
                SESSION_PUBLIC_INDEX_COMPACT_TEXT_MAX_BYTES,
            ),
            full_text,
        });
    }

    Ok(projected)
}

fn content_parts_text(parts: &[SessionContentPart]) -> String {
    let mut output = String::new();
    for part in parts {
        match part {
            SessionContentPart::Text { text } => output.push_str(text),
            SessionContentPart::Refusal { refusal } => output.push_str(refusal),
        }
    }
    output
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let marker = "…";
    let content_limit = max_bytes.saturating_sub(marker.len());
    let mut end = content_limit.min(value.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut truncated = value[..end].to_owned();
    if max_bytes >= marker.len() {
        truncated.push_str(marker);
    }
    truncated
}

fn persisted_entry_units(entries: &[LogEntry]) -> usize {
    entries.iter().fold(0_usize, |total, entry| {
        let embedded = match entry {
            LogEntry::AnnotatedSegmentStart { history, .. }
            | LogEntry::AnnotatedUserInput { history, .. } => history.len(),
            LogEntry::AnnotatedAssistantItem { .. }
            | LogEntry::AnnotatedToolResult { .. }
            | LogEntry::AnnotatedSystemItem { .. } => 1,
            _ => 0,
        };
        total.saturating_add(1).saturating_add(embedded)
    })
}

fn map_store_open_error(error: StoreError) -> SessionPublicIndexReadError {
    match error {
        StoreError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
            SessionPublicIndexReadError::Missing
        }
        StoreError::Corrupt { message, .. } if message.contains("requires migration") => {
            SessionPublicIndexReadError::MigrationRequired
        }
        StoreError::Io(_) => SessionPublicIndexReadError::Storage,
        StoreError::ReadLimitExceeded => SessionPublicIndexReadError::ResourceLimit,
        _ => SessionPublicIndexReadError::Corrupt,
    }
}

fn map_store_error(error: StoreError) -> SessionPublicIndexReadError {
    match error {
        StoreError::Corrupt { message, .. } if message.contains("requires migration") => {
            SessionPublicIndexReadError::MigrationRequired
        }
        StoreError::ReadLimitExceeded => SessionPublicIndexReadError::ResourceLimit,
        StoreError::Io(_) => SessionPublicIndexReadError::Storage,
        StoreError::NotFound(_) | StoreError::Serde(_) | StoreError::Corrupt { .. } => {
            SessionPublicIndexReadError::Corrupt
        }
        _ => SessionPublicIndexReadError::Corrupt,
    }
}

#[cfg(test)]
mod tests {
    use agen::EngineResult;
    use agen::llm_client::RequestConfig;
    use protocol::{InvokeKind, RunFailureKind, RunResumeSource, RunYieldReason};

    use super::*;
    use crate::{
        LoggedContentPart, LoggedHistoryEntry, LoggedItem, LoggedRole, LoggedSessionHistoryEntryId,
        LoggedSessionHistoryMetadata, LoggedSessionHistoryOrigin, LoggedSystemHistoryEntry, Store,
        SystemItem,
    };

    fn segment_id(value: u128) -> SegmentId {
        uuid::Uuid::from_u128(value)
    }

    fn history(
        id: &str,
        item: LoggedItem,
        origin: LoggedSessionHistoryOrigin,
    ) -> LoggedHistoryEntry {
        LoggedHistoryEntry {
            item,
            metadata: LoggedSessionHistoryMetadata {
                entry_id: LoggedSessionHistoryEntryId(id.to_owned()),
                origin,
                derivation: None,
            },
        }
    }

    fn message(id: &str, role: LoggedRole, text: &str) -> LoggedHistoryEntry {
        let origin = match role {
            LoggedRole::User => LoggedSessionHistoryOrigin::HumanInput {
                account_id: "account-1".into(),
            },
            LoggedRole::Assistant => LoggedSessionHistoryOrigin::ModelOutput {
                worker: crate::LoggedWorkerSubject {
                    workspace_id: Some("workspace-1".into()),
                    runtime_id: Some("runtime-1".into()),
                    worker_id: "worker-1".into(),
                },
            },
            LoggedRole::System => {
                LoggedSessionHistoryOrigin::BackendInstruction { operation_id: None }
            }
        };
        history(
            id,
            LoggedItem::Message {
                role,
                content: vec![LoggedContentPart::Text { text: text.into() }],
            },
            origin,
        )
    }

    fn start(
        session_id: SessionId,
        history: Vec<LoggedHistoryEntry>,
        forked_from: Option<SegmentOrigin>,
        compacted_from: Option<SegmentOrigin>,
    ) -> LogEntry {
        LogEntry::AnnotatedSegmentStart {
            ts: 1,
            session_id,
            system_prompt: Some("hidden system prompt".into()),
            config: RequestConfig::default(),
            history,
            forked_from,
            compacted_from,
        }
    }

    fn create_store() -> (tempfile::TempDir, std::path::PathBuf, WorkerSessionStore) {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("session");
        let store = WorkerSessionStore::new(&root).unwrap();
        (temp, root, store)
    }

    #[test]
    fn indexes_all_segments_in_order_and_keeps_nonactive_branches() {
        let (_temp, root, store) = create_store();
        let session_id = crate::new_session_id();
        let root_segment = segment_id(1);
        let fork_segment = segment_id(2);
        let compact_segment = segment_id(3);
        let seed = message("seed", LoggedRole::User, "seed text");

        store
            .create_segment(
                session_id,
                compact_segment,
                &[start(
                    session_id,
                    vec![seed.clone()],
                    None,
                    Some(SegmentOrigin {
                        segment_id: root_segment,
                        at_turn_index: 0,
                    }),
                )],
            )
            .unwrap();
        store
            .create_segment(
                session_id,
                root_segment,
                &[
                    start(session_id, vec![seed.clone()], None, None),
                    LogEntry::AnnotatedAssistantItem {
                        ts: 2,
                        entry: message("root-answer", LoggedRole::Assistant, "root answer"),
                    },
                ],
            )
            .unwrap();
        store
            .create_segment(
                session_id,
                fork_segment,
                &[
                    start(
                        session_id,
                        vec![seed],
                        Some(SegmentOrigin {
                            segment_id: root_segment,
                            at_turn_index: 0,
                        }),
                        None,
                    ),
                    LogEntry::AnnotatedAssistantItem {
                        ts: 3,
                        entry: message("branch-answer", LoggedRole::Assistant, "branch answer"),
                    },
                ],
            )
            .unwrap();

        let index = read_session_public_index(&root, SessionPublicIndexLimits::default()).unwrap();
        assert_eq!(
            index
                .segments
                .iter()
                .map(|segment| segment.segment_id.as_str())
                .collect::<Vec<_>>(),
            vec![
                root_segment.to_string(),
                fork_segment.to_string(),
                compact_segment.to_string(),
            ]
        );
        assert!(
            index.segments[0]
                .entries
                .iter()
                .any(|entry| entry.full_text == "root answer")
        );
        assert!(
            index.segments[1]
                .entries
                .iter()
                .any(|entry| entry.full_text == "branch answer")
        );
        assert_eq!(
            index.segments[1].lineage,
            SessionPublicIndexLineage {
                origin_kind: SessionPublicIndexOriginKind::Fork,
                parent_segment_id: Some(root_segment.to_string()),
                parent_turn_index: Some(0),
            }
        );
        assert_eq!(
            index.segments[2].lineage.origin_kind,
            SessionPublicIndexOriginKind::Compact
        );
        assert_eq!(index.segments[0].entries[0].entry_ref, "Eseed");
        assert_eq!(index.segments[1].entries[0].entry_ref, "Eseed");
        assert_eq!(index.segments[2].entries[0].entry_ref, "Eseed");
        assert!(
            index
                .segments
                .iter()
                .flat_map(|segment| &segment.entries)
                .all(|entry| entry.segment_id
                    == index
                        .segments
                        .iter()
                        .find(|segment| segment.entries.contains(entry))
                        .unwrap()
                        .segment_id)
        );
    }

    #[test]
    fn excludes_system_reasoning_and_attachment_bodies_and_maps_tool_output_name() {
        let (_temp, root, store) = create_store();
        let session_id = crate::new_session_id();
        let segment_id = segment_id(10);
        let worker = crate::LoggedWorkerSubject {
            workspace_id: Some("workspace-1".into()),
            runtime_id: Some("runtime-1".into()),
            worker_id: "worker-1".into(),
        };
        let log = vec![
            start(
                session_id,
                vec![
                    message("system", LoggedRole::System, "hidden system entry"),
                    message("user", LoggedRole::User, "visible user"),
                    history(
                        "reasoning",
                        LoggedItem::Reasoning {
                            text: "hidden reasoning".into(),
                            summary: vec!["hidden summary".into()],
                            encrypted_content: Some("hidden ciphertext".into()),
                            signature: None,
                        },
                        LoggedSessionHistoryOrigin::ModelOutput {
                            worker: worker.clone(),
                        },
                    ),
                    history(
                        "tool-call",
                        LoggedItem::ToolCall {
                            call_id: "call-1".into(),
                            name: "Read".into(),
                            arguments: "{\"path\":\"README.md\"}".into(),
                            call_index: Some(0),
                            execution_id: None,
                            status: None,
                        },
                        LoggedSessionHistoryOrigin::ModelOutput {
                            worker: worker.clone(),
                        },
                    ),
                    history(
                        "tool-output",
                        LoggedItem::ToolResult {
                            call_id: "call-1".into(),
                            summary: "read result".into(),
                            content: Some("visible output".into()),
                            attachments: vec![crate::logged_item::LoggedAttachment::Image {
                                mime_type: "image/png".into(),
                                data: b"secret attachment body".to_vec(),
                            }],
                            disposition: agen::tool::ToolResultDisposition::Success,
                            is_error: false,
                        },
                        LoggedSessionHistoryOrigin::ToolOutput { worker },
                    ),
                ],
                None,
                None,
            ),
            LogEntry::AnnotatedSystemItem {
                ts: 2,
                entry: LoggedSystemHistoryEntry {
                    item: SystemItem::Notification {
                        message: "hidden notification".into(),
                        body: "hidden system body".into(),
                        prompt_provenance: None,
                    },
                    metadata: LoggedSessionHistoryMetadata {
                        entry_id: LoggedSessionHistoryEntryId("system-item".into()),
                        origin: LoggedSessionHistoryOrigin::BackendInstruction {
                            operation_id: None,
                        },
                        derivation: None,
                    },
                },
                extensions: Vec::new(),
            },
        ];
        store.create_segment(session_id, segment_id, &log).unwrap();

        let index = read_session_public_index(&root, SessionPublicIndexLimits::default()).unwrap();
        let entries = &index.segments[0].entries;
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].entry_ref, "Euser");
        assert_eq!(entries[0].provenance, SessionEntryProvenance::HumanInput);
        assert_eq!(
            entries[1].tool_part,
            Some(SessionPublicIndexToolPart::Input)
        );
        assert_eq!(entries[1].tool_name.as_deref(), Some("Read"));
        assert_eq!(
            entries[2].tool_part,
            Some(SessionPublicIndexToolPart::Output)
        );
        assert_eq!(entries[2].tool_name.as_deref(), Some("Read"));
        assert_eq!(entries[2].compact_text, "read result");
        assert_eq!(entries[2].full_text, "visible output");
        let json = serde_json::to_string(&index).unwrap();
        for hidden in [
            "hidden system prompt",
            "hidden system entry",
            "hidden reasoning",
            "hidden ciphertext",
            "hidden notification",
            "hidden system body",
            "secret attachment body",
        ] {
            assert!(!json.contains(hidden), "public index leaked {hidden:?}");
        }
    }

    #[test]
    fn excludes_uncommitted_cancelled_and_failed_outputs_but_keeps_committed_outcomes() {
        let (_temp, root, store) = create_store();
        let session_id = crate::new_session_id();
        let outcomes = vec![
            (
                segment_id(20),
                "completed",
                LogEntry::RunCompleted {
                    ts: 4,
                    interrupted: false,
                    result: EngineResult::Finished,
                    active_run_turn_count: None,
                },
            ),
            (
                segment_id(21),
                "yielded",
                LogEntry::RunYielded {
                    ts: 4,
                    entry_id: Some(LoggedSessionHistoryEntryId("yielded-transition".into())),
                    reason: RunYieldReason::Compaction,
                    active_run_turn_count: 1,
                },
            ),
            (
                segment_id(22),
                "cancelled",
                LogEntry::RunCancelled {
                    ts: 4,
                    entry_id: Some(LoggedSessionHistoryEntryId("cancelled-transition".into())),
                },
            ),
            (
                segment_id(23),
                "errored",
                LogEntry::RunErrored {
                    ts: 4,
                    entry_id: Some(LoggedSessionHistoryEntryId("errored-transition".into())),
                    interrupted: false,
                    message: "run failed".into(),
                    failure: Some(RunFailureKind::Compaction),
                },
            ),
            (
                segment_id(24),
                "abandoned",
                LogEntry::PausedTurnAbandoned { ts: 4 },
            ),
        ];
        for (segment_id, text, outcome) in outcomes {
            store
                .create_segment(
                    session_id,
                    segment_id,
                    &[
                        start(
                            session_id,
                            vec![message(
                                &format!("seed-{text}"),
                                LoggedRole::User,
                                "committed seed",
                            )],
                            None,
                            None,
                        ),
                        LogEntry::Invoke {
                            ts: 2,
                            trigger: InvokeKind::UserSend,
                        },
                        LogEntry::AnnotatedAssistantItem {
                            ts: 3,
                            entry: message(text, LoggedRole::Assistant, text),
                        },
                        outcome,
                    ],
                )
                .unwrap();
        }
        let active_segment = segment_id(25);
        store
            .create_segment(
                session_id,
                active_segment,
                &[
                    start(
                        session_id,
                        vec![message("active-seed", LoggedRole::User, "committed seed")],
                        None,
                        None,
                    ),
                    LogEntry::Invoke {
                        ts: 2,
                        trigger: InvokeKind::UserSend,
                    },
                    LogEntry::AnnotatedAssistantItem {
                        ts: 3,
                        entry: message(
                            "uncommitted-output",
                            LoggedRole::Assistant,
                            "uncommitted active output",
                        ),
                    },
                ],
            )
            .unwrap();
        let resumed_segment = segment_id(26);
        store
            .create_segment(
                session_id,
                resumed_segment,
                &[
                    start(
                        session_id,
                        vec![message(
                            "yielded-seed",
                            LoggedRole::Assistant,
                            "committed yielded seed",
                        )],
                        None,
                        None,
                    ),
                    LogEntry::RunResumed {
                        ts: 2,
                        entry_id: Some(LoggedSessionHistoryEntryId("resumed-transition".into())),
                        source: RunResumeSource::Compaction,
                        active_run_turn_count: 1,
                    },
                    LogEntry::AnnotatedAssistantItem {
                        ts: 3,
                        entry: message(
                            "uncommitted-resume",
                            LoggedRole::Assistant,
                            "uncommitted resumed output",
                        ),
                    },
                ],
            )
            .unwrap();

        let index = read_session_public_index(&root, SessionPublicIndexLimits::default()).unwrap();
        let all_text: Vec<&str> = index
            .segments
            .iter()
            .flat_map(|segment| &segment.entries)
            .map(|entry| entry.full_text.as_str())
            .collect();
        for retained in ["completed", "yielded"] {
            assert!(all_text.contains(&retained));
        }
        for excluded in [
            "cancelled",
            "errored",
            "abandoned",
            "uncommitted active output",
            "uncommitted resumed output",
        ] {
            assert!(!all_text.contains(&excluded));
        }
        assert!(all_text.contains(&"committed seed"));
        assert!(all_text.contains(&"committed yielded seed"));
    }

    #[test]
    fn generation_is_stable_and_detects_append_replacement_and_segment_addition() {
        let (_temp, root, store) = create_store();
        let session_id = crate::new_session_id();
        let first_segment = segment_id(30);
        let initial = vec![start(
            session_id,
            vec![message("entry-1", LoggedRole::User, "one")],
            None,
            None,
        )];
        store
            .create_segment(session_id, first_segment, &initial)
            .unwrap();

        let first = read_session_public_index(&root, SessionPublicIndexLimits::default()).unwrap();
        let unchanged =
            read_session_public_index(&root, SessionPublicIndexLimits::default()).unwrap();
        assert_eq!(first.generation, unchanged.generation);

        store
            .append(
                session_id,
                first_segment,
                &LogEntry::AnnotatedAssistantItem {
                    ts: 2,
                    entry: message("entry-2", LoggedRole::Assistant, "two"),
                },
            )
            .unwrap();
        let appended =
            read_session_public_index(&root, SessionPublicIndexLimits::default()).unwrap();
        assert_ne!(first.generation, appended.generation);

        store
            .create_segment(
                session_id,
                first_segment,
                &[start(
                    session_id,
                    vec![message("entry-1", LoggedRole::User, "replacement")],
                    None,
                    None,
                )],
            )
            .unwrap();
        let replaced =
            read_session_public_index(&root, SessionPublicIndexLimits::default()).unwrap();
        assert_ne!(appended.generation, replaced.generation);

        store
            .create_segment(
                session_id,
                segment_id(31),
                &[start(session_id, Vec::new(), None, None)],
            )
            .unwrap();
        let added = read_session_public_index(&root, SessionPublicIndexLimits::default()).unwrap();
        assert_ne!(replaced.generation, added.generation);
    }

    #[test]
    fn enforces_byte_segment_and_entry_bounds_with_typed_errors() {
        let (_temp, root, store) = create_store();
        let session_id = crate::new_session_id();
        store
            .create_segment(
                session_id,
                segment_id(40),
                &[start(
                    session_id,
                    vec![message("entry-1", LoggedRole::User, "bounded")],
                    None,
                    None,
                )],
            )
            .unwrap();
        store
            .create_segment(
                session_id,
                segment_id(41),
                &[start(session_id, Vec::new(), None, None)],
            )
            .unwrap();

        assert_eq!(
            read_session_public_index(
                &root,
                SessionPublicIndexLimits {
                    max_segments: 1,
                    ..SessionPublicIndexLimits::default()
                },
            ),
            Err(SessionPublicIndexReadError::ResourceLimit)
        );
        assert_eq!(
            read_session_public_index(
                &root,
                SessionPublicIndexLimits {
                    max_entries: 1,
                    ..SessionPublicIndexLimits::default()
                },
            ),
            Err(SessionPublicIndexReadError::ResourceLimit)
        );
        assert_eq!(
            read_session_public_index(
                &root,
                SessionPublicIndexLimits {
                    max_bytes: 1,
                    ..SessionPublicIndexLimits::default()
                },
            ),
            Err(SessionPublicIndexReadError::ResourceLimit)
        );
        assert_eq!(
            read_session_public_index(&root.join("missing"), SessionPublicIndexLimits::default(),),
            Err(SessionPublicIndexReadError::Missing)
        );
    }

    #[test]
    fn rejects_invalid_session_and_lineage_origins() {
        let (_temp, root, store) = create_store();
        let session_id = crate::new_session_id();
        let root_segment = segment_id(50);
        let child_segment = segment_id(51);
        store
            .create_segment(
                session_id,
                root_segment,
                &[start(session_id, Vec::new(), None, None)],
            )
            .unwrap();
        store
            .create_segment(
                session_id,
                child_segment,
                &[start(
                    session_id,
                    Vec::new(),
                    Some(SegmentOrigin {
                        segment_id: root_segment,
                        at_turn_index: 1,
                    }),
                    None,
                )],
            )
            .unwrap();
        assert_eq!(
            read_session_public_index(&root, SessionPublicIndexLimits::default()),
            Err(SessionPublicIndexReadError::Corrupt)
        );
    }

    #[test]
    fn reports_migration_without_rewriting_the_manifest() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("session");
        std::fs::create_dir_all(root.join("segments")).unwrap();
        let session_id = crate::new_session_id();
        let manifest = format!("{{\"schema_version\":2,\"session_id\":\"{session_id}\"}}");
        std::fs::write(root.join("session.json"), &manifest).unwrap();

        assert_eq!(
            read_session_public_index(&root, SessionPublicIndexLimits::default()),
            Err(SessionPublicIndexReadError::MigrationRequired)
        );
        assert_eq!(
            std::fs::read_to_string(root.join("session.json")).unwrap(),
            manifest
        );
    }
}
