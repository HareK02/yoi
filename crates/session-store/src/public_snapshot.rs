use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD},
};
use protocol::{
    Segment, SessionContentPart, SessionConversationTurn, SessionEntryProvenance,
    SessionHistoryPage, SessionMessageRole, SessionSnapshot, SessionSnapshotEntry,
    SessionSnapshotEntryData, SessionToolAttachment,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use std::collections::{HashMap, HashSet};
use std::path::Path;

use thiserror::Error;

use crate::{
    LogEntry, LoggedContentPart, LoggedHistoryEntry, LoggedItem, LoggedRole,
    LoggedSessionHistoryOrigin, SegmentId, SegmentOrigin, SessionId, StoreError, SystemItem,
    WorkerAggregateStore, WorkerSessionStore, WorkerStoreError,
};

/// Projection limit leaves one MiB for the typed API envelope so a public
/// retained-session response remains below the 16 MiB response ceiling.
pub const DEFAULT_RETAINED_SNAPSHOT_MAX_BYTES: u64 = 15 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedSessionIdentity {
    pub session_id: String,
    pub segment_id: String,
    pub entry_count: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RetainedSessionSnapshot {
    pub identity: RetainedSessionIdentity,
    pub snapshot: SessionSnapshot,
}

#[derive(Debug, Error)]
pub enum RetainedSnapshotReadError {
    #[error("retained worker state is unavailable")]
    RetentionMissing,
    #[error("retained worker state has no active session pointer")]
    ActivePointerMissing,
    #[error("retained session requires migration")]
    MigrationRequired,
    #[error("retained session log is corrupt")]
    CorruptLog,
    #[error("retained session storage is unavailable")]
    StorageUnavailable,
    #[error("retained session snapshot exceeds the observation limit")]
    SnapshotTooLarge,
}

pub const DEFAULT_RETAINED_HISTORY_PAGE_TURNS: usize = 5;
pub const MAX_RETAINED_HISTORY_PAGE_TURNS: usize = 5;
pub const DEFAULT_RETAINED_HISTORY_MAX_SCAN_BYTES: u64 = 64 * 1024 * 1024;
pub const DEFAULT_RETAINED_HISTORY_MAX_SEGMENTS: usize = 64;
pub const DEFAULT_RETAINED_HISTORY_MAX_ENTRIES: usize = 100_000;
pub const DEFAULT_RETAINED_HISTORY_MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;

/// Explicit resource bounds for one retained-history page read.
///
/// The operation never truncates a turn to fit these limits. A lineage or one
/// selected page that exceeds a bound fails with [`RetainedHistoryReadError::ResourceLimit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetainedHistoryReadLimits {
    pub max_scan_bytes: u64,
    pub max_segments: usize,
    pub max_entries: usize,
    pub max_response_bytes: u64,
}

impl Default for RetainedHistoryReadLimits {
    fn default() -> Self {
        Self {
            max_scan_bytes: DEFAULT_RETAINED_HISTORY_MAX_SCAN_BYTES,
            max_segments: DEFAULT_RETAINED_HISTORY_MAX_SEGMENTS,
            max_entries: DEFAULT_RETAINED_HISTORY_MAX_ENTRIES,
            max_response_bytes: DEFAULT_RETAINED_HISTORY_MAX_RESPONSE_BYTES,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RetainedHistoryReadError {
    #[error("retained worker state is unavailable")]
    RetentionMissing,
    #[error("retained worker state has no active session pointer")]
    ActivePointerMissing,
    #[error("retained session requires migration")]
    MigrationRequired,
    #[error("retained session log is corrupt")]
    CorruptLog,
    #[error("retained session storage is unavailable")]
    StorageUnavailable,
    #[error("retained session history cursor is invalid or stale")]
    InvalidCursor,
    #[error("retained session history exceeds a resource limit")]
    ResourceLimit,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetainedHistoryCursor {
    version: u8,
    worker_name: String,
    session_id: String,
    lineage_id: String,
    before_turn_id: String,
}

#[derive(Debug)]
struct LoadedLineageSegment {
    segment_id: SegmentId,
    adopted_through_turn: Option<usize>,
    entries: Vec<LogEntry>,
}

#[derive(Debug, Clone, Copy)]
enum LineageOriginKind {
    Fork,
    Compact,
}

#[derive(Debug)]
struct LineageOrigin {
    kind: LineageOriginKind,
    origin: SegmentOrigin,
}

/// Read the active retained Session without creating, migrating, restoring, or
/// rewriting any persisted state.
pub fn read_retained_session_snapshot(
    aggregate_root: &Path,
    worker_name: &str,
    max_bytes: u64,
) -> Result<RetainedSessionSnapshot, RetainedSnapshotReadError> {
    let aggregate = WorkerAggregateStore::open_read_only(aggregate_root, worker_name)
        .map_err(map_worker_store_error)?;
    let metadata = aggregate
        .read_read_only()
        .map_err(map_worker_store_error)?
        .ok_or(RetainedSnapshotReadError::RetentionMissing)?;
    let active = metadata
        .active
        .ok_or(RetainedSnapshotReadError::ActivePointerMissing)?;
    let segment_id = active
        .segment_id
        .ok_or(RetainedSnapshotReadError::ActivePointerMissing)?;
    let store = WorkerSessionStore::open_read_only(aggregate_root.join("session"))
        .map_err(map_store_error)?;
    if store.segment_log_len(segment_id).map_err(map_store_error)? > max_bytes {
        return Err(RetainedSnapshotReadError::SnapshotTooLarge);
    }
    let entries = store
        .read_all_read_only(active.session_id, segment_id)
        .map_err(map_store_error)?;
    let snapshot = project_session_snapshot(active.session_id, &entries);
    if serde_json::to_vec(&snapshot)
        .map_err(|_| RetainedSnapshotReadError::CorruptLog)?
        .len() as u64
        > max_bytes
    {
        return Err(RetainedSnapshotReadError::SnapshotTooLarge);
    }
    Ok(RetainedSessionSnapshot {
        identity: RetainedSessionIdentity {
            session_id: active.session_id.to_string(),
            segment_id: segment_id.to_string(),
            entry_count: entries.len() as u64,
        },
        snapshot,
    })
}

/// Read one bounded backward page from the retained active Segment's adopted
/// lineage without creating, migrating, restoring, or rewriting persisted state.
///
/// `cursor` is an opaque boundary returned by the previous page. It is bound to
/// the Worker name, Session, active lineage, and stable user-entry identity.
pub fn read_retained_session_history_page(
    aggregate_root: &Path,
    worker_name: &str,
    cursor: Option<&str>,
    limit: Option<usize>,
    limits: RetainedHistoryReadLimits,
) -> Result<SessionHistoryPage, RetainedHistoryReadError> {
    let page_limit = limit.unwrap_or(DEFAULT_RETAINED_HISTORY_PAGE_TURNS);
    if page_limit == 0 || page_limit > MAX_RETAINED_HISTORY_PAGE_TURNS {
        return Err(RetainedHistoryReadError::ResourceLimit);
    }

    let aggregate = WorkerAggregateStore::open_read_only(aggregate_root, worker_name)
        .map_err(map_history_worker_store_error)?;
    let metadata = aggregate
        .read_read_only()
        .map_err(map_history_worker_store_error)?
        .ok_or(RetainedHistoryReadError::RetentionMissing)?;
    let active = metadata
        .active
        .ok_or(RetainedHistoryReadError::ActivePointerMissing)?;
    let active_segment_id = active
        .segment_id
        .ok_or(RetainedHistoryReadError::ActivePointerMissing)?;
    let store = WorkerSessionStore::open_read_only(aggregate_root.join("session"))
        .map_err(map_history_store_error)?;
    let (lineage, lineage_id) =
        load_adopted_lineage(&store, active.session_id, active_segment_id, limits)?;
    let entries = project_adopted_lineage(active.session_id, &lineage)?;
    let turns = group_conversation_turns(entries);

    let page_end = match cursor {
        Some(cursor) => {
            let cursor = decode_history_cursor(cursor)?;
            if cursor.version != 1
                || cursor.worker_name != worker_name
                || cursor.session_id != active.session_id.to_string()
                || cursor.lineage_id != lineage_id
            {
                return Err(RetainedHistoryReadError::InvalidCursor);
            }
            turns
                .iter()
                .position(|turn| turn.turn_id == cursor.before_turn_id)
                .ok_or(RetainedHistoryReadError::InvalidCursor)?
        }
        None => turns.len(),
    };
    let page_start = page_end.saturating_sub(page_limit);
    let page_turns = turns[page_start..page_end].to_vec();
    let has_more = page_start > 0;
    let next_cursor = has_more
        .then(|| {
            encode_history_cursor(&RetainedHistoryCursor {
                version: 1,
                worker_name: worker_name.to_owned(),
                session_id: active.session_id.to_string(),
                lineage_id: lineage_id.clone(),
                before_turn_id: page_turns
                    .first()
                    .expect("a page with earlier history cannot be empty")
                    .turn_id
                    .clone(),
            })
        })
        .transpose()?;
    let page = SessionHistoryPage {
        session_id: active.session_id.to_string(),
        lineage_id,
        turns: page_turns,
        next_cursor,
        has_more,
    };
    let response_bytes = serde_json::to_vec(&page)
        .map_err(|_| RetainedHistoryReadError::CorruptLog)?
        .len() as u64;
    if response_bytes > limits.max_response_bytes {
        return Err(RetainedHistoryReadError::ResourceLimit);
    }
    Ok(page)
}

fn load_adopted_lineage(
    store: &WorkerSessionStore,
    session_id: SessionId,
    active_segment_id: SegmentId,
    limits: RetainedHistoryReadLimits,
) -> Result<(Vec<LoadedLineageSegment>, String), RetainedHistoryReadError> {
    let mut lineage = Vec::new();
    let mut visited = HashSet::new();
    let mut segment_id = active_segment_id;
    let mut adopted_through_turn = None;
    let mut scanned_bytes = 0_u64;
    let mut scanned_entries = 0_usize;
    let mut lineage_hash = Sha256::new();
    lineage_hash.update(b"yoi-retained-history-lineage-v1\0");
    lineage_hash.update(session_id.as_bytes());
    lineage_hash.update(active_segment_id.as_bytes());

    loop {
        if lineage.len() >= limits.max_segments {
            return Err(RetainedHistoryReadError::ResourceLimit);
        }
        if !visited.insert(segment_id) {
            return Err(RetainedHistoryReadError::CorruptLog);
        }
        let remaining_scan_bytes = limits
            .max_scan_bytes
            .checked_sub(scanned_bytes)
            .ok_or(RetainedHistoryReadError::ResourceLimit)?;
        let (entries, segment_bytes) = store
            .read_all_read_only_bounded(session_id, segment_id, remaining_scan_bytes)
            .map_err(map_history_store_error)?;
        scanned_bytes = scanned_bytes
            .checked_add(segment_bytes)
            .ok_or(RetainedHistoryReadError::ResourceLimit)?;
        scanned_entries = scanned_entries
            .checked_add(persisted_entry_units(&entries))
            .ok_or(RetainedHistoryReadError::ResourceLimit)?;
        if scanned_entries > limits.max_entries {
            return Err(RetainedHistoryReadError::ResourceLimit);
        }
        let origin = validate_segment_start(session_id, &entries)?;

        lineage_hash.update(segment_id.as_bytes());
        match adopted_through_turn {
            Some(turn) => {
                lineage_hash.update([1]);
                lineage_hash.update((turn as u64).to_be_bytes());
            }
            None => lineage_hash.update([0]),
        }
        if let Some(origin) = &origin {
            lineage_hash.update(match origin.kind {
                LineageOriginKind::Fork => [1],
                LineageOriginKind::Compact => [2],
            });
            lineage_hash.update(origin.origin.segment_id.as_bytes());
            lineage_hash.update((origin.origin.at_turn_index as u64).to_be_bytes());
        } else {
            lineage_hash.update([0]);
        }

        lineage.push(LoadedLineageSegment {
            segment_id,
            adopted_through_turn,
            entries,
        });
        let Some(origin) = origin else {
            break;
        };
        segment_id = origin.origin.segment_id;
        adopted_through_turn = Some(
            adopted_through_turn
                .map(|boundary| boundary.min(origin.origin.at_turn_index))
                .unwrap_or(origin.origin.at_turn_index),
        );
    }

    lineage.reverse();
    Ok((lineage, URL_SAFE_NO_PAD.encode(lineage_hash.finalize())))
}

fn validate_segment_start(
    expected_session_id: SessionId,
    entries: &[LogEntry],
) -> Result<Option<LineageOrigin>, RetainedHistoryReadError> {
    if entries.is_empty() {
        // A freshly reserved active Segment can be empty until its first turn
        // materializes SegmentStart. The Session manifest still supplies the
        // same-Session fence, and an empty Segment has no parent lineage.
        return Ok(None);
    }
    let Some(LogEntry::AnnotatedSegmentStart {
        session_id,
        forked_from,
        compacted_from,
        ..
    }) = entries.first()
    else {
        return Err(RetainedHistoryReadError::CorruptLog);
    };
    if *session_id != expected_session_id
        || entries
            .iter()
            .skip(1)
            .any(|entry| matches!(entry, LogEntry::AnnotatedSegmentStart { .. }))
    {
        return Err(RetainedHistoryReadError::CorruptLog);
    }
    match (forked_from, compacted_from) {
        (Some(_), Some(_)) => Err(RetainedHistoryReadError::CorruptLog),
        (Some(origin), None) => Ok(Some(LineageOrigin {
            kind: LineageOriginKind::Fork,
            origin: origin.clone(),
        })),
        (None, Some(origin)) => Ok(Some(LineageOrigin {
            kind: LineageOriginKind::Compact,
            origin: origin.clone(),
        })),
        (None, None) => Ok(None),
    }
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

fn project_adopted_lineage(
    session_id: SessionId,
    lineage: &[LoadedLineageSegment],
) -> Result<Vec<SessionSnapshotEntry>, RetainedHistoryReadError> {
    let mut entries = Vec::<SessionSnapshotEntry>::new();
    let mut positions = HashMap::<String, usize>::new();
    let mut excluded_by_boundary = HashSet::<String>::new();

    for segment in lineage {
        let prefix = adopted_segment_prefix(&segment.entries, segment.adopted_through_turn)?;
        let snapshot =
            project_session_snapshot_for_segment(session_id, Some(segment.segment_id), prefix);
        if prefix.len() < segment.entries.len() {
            let adopted_ids = snapshot
                .entries
                .iter()
                .map(|entry| entry.entry_id.as_str())
                .collect::<HashSet<_>>();
            for entry in project_session_snapshot_for_segment(
                session_id,
                Some(segment.segment_id),
                &segment.entries,
            )
            .entries
            {
                if !adopted_ids.contains(entry.entry_id.as_str()) {
                    excluded_by_boundary.insert(entry.entry_id);
                }
            }
        }
        for mut entry in snapshot.entries {
            if entry.provenance == SessionEntryProvenance::DerivedSummary
                || excluded_by_boundary.contains(&entry.entry_id)
            {
                continue;
            }
            if let Some(position) = positions.get(&entry.entry_id).copied() {
                // A newer seed may carry a richer canonical projection (notably
                // typed input segments). Keep the original durable timestamp and
                // ordering while replacing the duplicate representation.
                entry.timestamp = entries[position].timestamp;
                entries[position] = entry;
            } else {
                positions.insert(entry.entry_id.clone(), entries.len());
                entries.push(entry);
            }
        }
    }
    Ok(entries)
}

fn adopted_segment_prefix(
    entries: &[LogEntry],
    adopted_through_turn: Option<usize>,
) -> Result<&[LogEntry], RetainedHistoryReadError> {
    let Some(boundary) = adopted_through_turn else {
        return Ok(entries);
    };
    let seed_end = entries
        .iter()
        .position(|entry| {
            !matches!(
                entry,
                LogEntry::AnnotatedSegmentStart { .. } | LogEntry::InputSegmentsCheckpoint { .. }
            )
        })
        .unwrap_or(entries.len());
    if boundary == 0 {
        return Ok(&entries[..seed_end]);
    }

    let mut current_turn_start = seed_end;
    for (index, entry) in entries.iter().enumerate().skip(seed_end) {
        match entry {
            LogEntry::Invoke { .. } => current_turn_start = index,
            LogEntry::TurnEnd { turn_count, .. } if *turn_count == boundary => {
                return Ok(&entries[..=index]);
            }
            LogEntry::TurnEnd { turn_count, .. } if *turn_count > boundary => {
                return Ok(&entries[..current_turn_start]);
            }
            _ => {}
        }
    }
    Ok(entries)
}

fn group_conversation_turns(entries: Vec<SessionSnapshotEntry>) -> Vec<SessionConversationTurn> {
    let mut turns = Vec::new();
    let mut current: Option<SessionConversationTurn> = None;
    for entry in entries {
        if is_user_entry(&entry) {
            if let Some(turn) = current.take() {
                turns.push(turn);
            }
            current = Some(SessionConversationTurn {
                turn_id: entry.entry_id.clone(),
                entries: vec![entry],
            });
        } else if let Some(turn) = current.as_mut() {
            turn.entries.push(entry);
        }
    }
    if let Some(turn) = current {
        turns.push(turn);
    }
    turns
}

fn is_user_entry(entry: &SessionSnapshotEntry) -> bool {
    matches!(
        &entry.data,
        SessionSnapshotEntryData::UserInput { .. }
            | SessionSnapshotEntryData::Message {
                role: SessionMessageRole::User,
                ..
            }
    )
}

fn encode_history_cursor(
    cursor: &RetainedHistoryCursor,
) -> Result<String, RetainedHistoryReadError> {
    serde_json::to_vec(cursor)
        .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
        .map_err(|_| RetainedHistoryReadError::CorruptLog)
}

fn decode_history_cursor(cursor: &str) -> Result<RetainedHistoryCursor, RetainedHistoryReadError> {
    const MAX_CURSOR_BYTES: usize = 4096;
    if cursor.is_empty() || cursor.len() > MAX_CURSOR_BYTES {
        return Err(RetainedHistoryReadError::InvalidCursor);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| RetainedHistoryReadError::InvalidCursor)?;
    if bytes.len() > MAX_CURSOR_BYTES {
        return Err(RetainedHistoryReadError::InvalidCursor);
    }
    serde_json::from_slice(&bytes).map_err(|_| RetainedHistoryReadError::InvalidCursor)
}

fn map_history_worker_store_error(error: WorkerStoreError) -> RetainedHistoryReadError {
    match error {
        WorkerStoreError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
            RetainedHistoryReadError::RetentionMissing
        }
        WorkerStoreError::Serde(_) | WorkerStoreError::InvalidWorkerName(_) => {
            RetainedHistoryReadError::CorruptLog
        }
        WorkerStoreError::Io(_) => RetainedHistoryReadError::StorageUnavailable,
    }
}

fn map_history_store_error(error: StoreError) -> RetainedHistoryReadError {
    match error {
        StoreError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
            RetainedHistoryReadError::RetentionMissing
        }
        StoreError::Corrupt { message, .. } if message.contains("requires migration") => {
            RetainedHistoryReadError::MigrationRequired
        }
        StoreError::ReadLimitExceeded => RetainedHistoryReadError::ResourceLimit,
        StoreError::Io(_) => RetainedHistoryReadError::StorageUnavailable,
        StoreError::Serde(_) | StoreError::Corrupt { .. } | StoreError::NotFound(_) => {
            RetainedHistoryReadError::CorruptLog
        }
        _ => RetainedHistoryReadError::CorruptLog,
    }
}

fn map_worker_store_error(error: WorkerStoreError) -> RetainedSnapshotReadError {
    match error {
        WorkerStoreError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
            RetainedSnapshotReadError::RetentionMissing
        }
        WorkerStoreError::Serde(_) | WorkerStoreError::InvalidWorkerName(_) => {
            RetainedSnapshotReadError::CorruptLog
        }
        WorkerStoreError::Io(_) => RetainedSnapshotReadError::StorageUnavailable,
    }
}

fn map_store_error(error: StoreError) -> RetainedSnapshotReadError {
    match error {
        StoreError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
            RetainedSnapshotReadError::RetentionMissing
        }
        StoreError::Corrupt { message, .. } if message.contains("requires migration") => {
            RetainedSnapshotReadError::MigrationRequired
        }
        StoreError::Io(_) => RetainedSnapshotReadError::StorageUnavailable,
        StoreError::Serde(_) | StoreError::Corrupt { .. } => RetainedSnapshotReadError::CorruptLog,
        _ => RetainedSnapshotReadError::CorruptLog,
    }
}

/// Project a complete current-segment log. A valid segment starts with one
/// canonical annotated SegmentStart record; malformed partial input uses the
/// nil session only to keep the public failure projection deterministic.
pub fn project_current_session_snapshot(log: &[LogEntry]) -> SessionSnapshot {
    let session_id = log.iter().find_map(|entry| match entry {
        LogEntry::AnnotatedSegmentStart { session_id, .. } => Some(*session_id),
        _ => None,
    });
    project_session_snapshot(session_id.unwrap_or_else(SessionId::nil), log)
}

/// Project the current durable segment into the only public session-history
/// representation. Append-log records remain an internal persistence format.
pub fn project_session_snapshot(session_id: SessionId, log: &[LogEntry]) -> SessionSnapshot {
    project_session_snapshot_for_segment(session_id, None, log)
}

fn project_session_snapshot_for_segment(
    session_id: SessionId,
    segment_id: Option<SegmentId>,
    log: &[LogEntry],
) -> SessionSnapshot {
    let mut session_key = session_id;
    let mut entries = Vec::new();

    for (log_index, record) in log.iter().enumerate() {
        match record {
            LogEntry::AnnotatedSegmentStart {
                ts,
                session_id,
                history,
                ..
            } => {
                session_key = *session_id;
                entries.clear();
                extend_history(&mut entries, history, None, *ts);
            }
            LogEntry::InputSegmentsCheckpoint { user_segments, .. } => {
                let mut segments = user_segments.iter();
                for entry in &mut entries {
                    let is_user = matches!(
                        &entry.data,
                        SessionSnapshotEntryData::UserInput { .. }
                            | SessionSnapshotEntryData::Message {
                                role: SessionMessageRole::User,
                                ..
                            }
                    );
                    if is_user && let Some(checkpoint) = segments.next() {
                        entry.data = SessionSnapshotEntryData::UserInput {
                            segments: checkpoint.clone(),
                        };
                    }
                }
            }
            LogEntry::AnnotatedUserInput {
                ts,
                segments,
                history,
                ..
            } => extend_history(&mut entries, history, Some(segments), *ts),
            LogEntry::AnnotatedAssistantItem { ts, entry }
            | LogEntry::AnnotatedToolResult { ts, entry } => {
                if let Some(data) = project_item(&entry.item) {
                    entries.push(history_entry(entry, *ts, data));
                }
            }
            LogEntry::AnnotatedSystemItem { ts, entry, .. } => entries.push(system_entry(
                &entry.item,
                entry.metadata.entry_id.0.clone(),
                *ts,
                provenance(&entry.metadata.origin),
                derivation_ids(entry),
            )),
            LogEntry::RunErrored { ts, message, .. } => entries.push(legacy_entry(
                &session_key,
                segment_id.as_ref(),
                log_index,
                0,
                *ts,
                SessionSnapshotEntryData::RunError {
                    message: message.clone(),
                },
            )),
            // Run checkpoints, configuration, usage, and extension state are
            // controller/storage authority rather than committed conversation.
            LogEntry::Invoke { .. }
            | LogEntry::TurnEnd { .. }
            | LogEntry::RunCompleted { .. }
            | LogEntry::ActiveRunCheckpoint { .. }
            | LogEntry::PausedTurnAbandoned { .. }
            | LogEntry::ConfigChanged { .. }
            | LogEntry::LlmUsage { .. }
            | LogEntry::Extension { .. } => {}
        }
    }

    SessionSnapshot {
        pending_submissions: protocol::PendingSubmissionsSnapshot::default(),
        entries,
    }
}

fn extend_history(
    output: &mut Vec<SessionSnapshotEntry>,
    history: &[LoggedHistoryEntry],
    input_segments: Option<&Vec<Segment>>,
    timestamp: u64,
) {
    let mut attached_segments = false;
    for entry in history {
        let data = if !attached_segments
            && input_segments.is_some()
            && matches!(
                &entry.item,
                LoggedItem::Message {
                    role: LoggedRole::User,
                    ..
                }
            ) {
            attached_segments = true;
            SessionSnapshotEntryData::UserInput {
                segments: input_segments.cloned().unwrap_or_default(),
            }
        } else {
            let Some(data) = project_item(&entry.item) else {
                continue;
            };
            data
        };
        output.push(history_entry(entry, timestamp, data));
    }
}

fn history_entry(
    entry: &LoggedHistoryEntry,
    timestamp: u64,
    data: SessionSnapshotEntryData,
) -> SessionSnapshotEntry {
    SessionSnapshotEntry {
        entry_id: entry.metadata.entry_id.0.clone(),
        timestamp,
        provenance: provenance(&entry.metadata.origin),
        derived_from: entry
            .metadata
            .derivation
            .as_ref()
            .map(|derivation| {
                derivation
                    .sources
                    .iter()
                    .map(|source| source.0.clone())
                    .collect()
            })
            .unwrap_or_default(),
        data,
    }
}

fn derivation_ids(entry: &crate::LoggedSystemHistoryEntry) -> Vec<String> {
    entry
        .metadata
        .derivation
        .as_ref()
        .map(|derivation| {
            derivation
                .sources
                .iter()
                .map(|source| source.0.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn legacy_entry(
    session_key: &SessionId,
    segment_id: Option<&SegmentId>,
    log_index: usize,
    item_index: usize,
    timestamp: u64,
    data: SessionSnapshotEntryData,
) -> SessionSnapshotEntry {
    SessionSnapshotEntry {
        entry_id: legacy_entry_id(session_key, segment_id, log_index, item_index),
        timestamp,
        provenance: SessionEntryProvenance::LegacyUnknown,
        derived_from: Vec::new(),
        data,
    }
}

fn legacy_entry_id(
    session_key: &SessionId,
    segment_id: Option<&SegmentId>,
    log_index: usize,
    item_index: usize,
) -> String {
    let mut identity = Vec::with_capacity(48);
    identity.extend_from_slice(session_key.as_bytes());
    if let Some(segment_id) = segment_id {
        identity.extend_from_slice(segment_id.as_bytes());
    }
    identity.extend_from_slice(&(log_index as u64).to_be_bytes());
    identity.extend_from_slice(&(item_index as u64).to_be_bytes());
    format!("l-{}", URL_SAFE_NO_PAD.encode(identity))
}

fn provenance(origin: &LoggedSessionHistoryOrigin) -> SessionEntryProvenance {
    match origin {
        LoggedSessionHistoryOrigin::HumanInput { .. } => SessionEntryProvenance::HumanInput,
        LoggedSessionHistoryOrigin::WorkerInput { .. } => SessionEntryProvenance::WorkerInput,
        LoggedSessionHistoryOrigin::FlowInstruction { .. } => {
            SessionEntryProvenance::FlowInstruction
        }
        LoggedSessionHistoryOrigin::BackendInstruction { .. } => {
            SessionEntryProvenance::BackendInstruction
        }
        LoggedSessionHistoryOrigin::ModelOutput { .. } => SessionEntryProvenance::ModelOutput,
        LoggedSessionHistoryOrigin::ToolOutput { .. } => SessionEntryProvenance::ToolOutput,
        LoggedSessionHistoryOrigin::DerivedSummary => SessionEntryProvenance::DerivedSummary,
        LoggedSessionHistoryOrigin::LegacyUnknown => SessionEntryProvenance::LegacyUnknown,
    }
}

fn project_item(item: &LoggedItem) -> Option<SessionSnapshotEntryData> {
    match item {
        LoggedItem::Message { role, content } => {
            let role = match role {
                LoggedRole::User => SessionMessageRole::User,
                LoggedRole::Assistant => SessionMessageRole::Assistant,
                // System prompts and instruction history never cross the public
                // snapshot boundary. Typed SystemItems have separate records.
                LoggedRole::System => return None,
            };
            Some(SessionSnapshotEntryData::Message {
                role,
                content: content
                    .iter()
                    .map(|part| match part {
                        LoggedContentPart::Text { text } => {
                            SessionContentPart::Text { text: text.clone() }
                        }
                        LoggedContentPart::Refusal { refusal } => SessionContentPart::Refusal {
                            refusal: refusal.clone(),
                        },
                    })
                    .collect(),
            })
        }
        LoggedItem::ToolCall {
            call_id,
            name,
            arguments,
            ..
        } => Some(SessionSnapshotEntryData::ToolCall {
            call_id: call_id.clone(),
            name: name.clone(),
            arguments: arguments.clone(),
        }),
        LoggedItem::ToolResult {
            call_id,
            summary,
            content,
            is_error,
            attachments,
            ..
        } => Some(SessionSnapshotEntryData::ToolResult {
            call_id: call_id.clone(),
            summary: summary.clone(),
            content: content.clone(),
            is_error: *is_error,
            attachments: attachments
                .iter()
                .map(|attachment| match attachment {
                    crate::logged_item::LoggedAttachment::Image { mime_type, data } => {
                        SessionToolAttachment {
                            media_type: mime_type.clone(),
                            data_base64: BASE64.encode(data),
                        }
                    }
                })
                .collect(),
        }),
        // Internal response grouping and hidden model reasoning are never observable.
        LoggedItem::AssistantResponseBoundary { .. } | LoggedItem::Reasoning { .. } => None,
    }
}

fn system_entry(
    item: &SystemItem,
    entry_id: String,
    timestamp: u64,
    provenance: SessionEntryProvenance,
    derived_from: Vec<String>,
) -> SessionSnapshotEntry {
    let mut data = serde_json::to_value(item).ok();
    if let Some(serde_json::Value::Object(object)) = data.as_mut() {
        object.remove("prompt_provenance");
    }
    let item_kind = data
        .as_ref()
        .and_then(|value| value.get("kind"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("system_item")
        .to_owned();
    SessionSnapshotEntry {
        entry_id,
        timestamp,
        provenance,
        derived_from,
        data: SessionSnapshotEntryData::SystemItem {
            item_kind,
            content: item.history_text(),
            data,
        },
    }
}

#[cfg(test)]
mod tests {
    use agen::llm_client::RequestConfig;
    use protocol::InvokeKind;

    use super::*;
    use crate::{
        LoggedHistoryDerivation, LoggedSessionHistoryEntryId, LoggedSessionHistoryMetadata,
        LoggedWorkerSubject, Store, WorkerMetadataStore,
    };

    fn history_message(
        entry_id: impl Into<String>,
        role: LoggedRole,
        text: impl Into<String>,
        origin: LoggedSessionHistoryOrigin,
    ) -> LoggedHistoryEntry {
        LoggedHistoryEntry {
            item: LoggedItem::Message {
                role,
                content: vec![LoggedContentPart::Text { text: text.into() }],
            },
            metadata: LoggedSessionHistoryMetadata {
                entry_id: LoggedSessionHistoryEntryId(entry_id.into()),
                origin,
                derivation: None,
            },
        }
    }

    fn user_message(turn: usize) -> LoggedHistoryEntry {
        history_message(
            format!("user-{turn}"),
            LoggedRole::User,
            format!("request {turn}"),
            LoggedSessionHistoryOrigin::HumanInput {
                account_id: "account-1".into(),
            },
        )
    }

    fn assistant_message(turn: usize, suffix: &str) -> LoggedHistoryEntry {
        history_message(
            format!("assistant-{turn}-{suffix}"),
            LoggedRole::Assistant,
            format!("response {turn} {suffix}"),
            LoggedSessionHistoryOrigin::LegacyUnknown,
        )
    }

    fn append_turn(log: &mut Vec<LogEntry>, turn: usize) {
        log.push(LogEntry::Invoke {
            ts: turn as u64 * 10,
            trigger: InvokeKind::UserSend,
        });
        log.push(LogEntry::AnnotatedUserInput {
            ts: turn as u64 * 10 + 1,
            segments: vec![Segment::Text {
                content: format!("request {turn}"),
            }],
            history: vec![user_message(turn)],
            extensions: Vec::new(),
        });
        log.push(LogEntry::AnnotatedAssistantItem {
            ts: turn as u64 * 10 + 2,
            entry: assistant_message(turn, "final"),
        });
        log.push(LogEntry::TurnEnd {
            ts: turn as u64 * 10 + 3,
            turn_count: turn,
        });
    }

    fn segment_start(
        session_id: SessionId,
        history: Vec<LoggedHistoryEntry>,
        forked_from: Option<SegmentOrigin>,
        compacted_from: Option<SegmentOrigin>,
    ) -> LogEntry {
        LogEntry::AnnotatedSegmentStart {
            ts: 1,
            session_id,
            system_prompt: None,
            config: RequestConfig::default(),
            history,
            forked_from,
            compacted_from,
        }
    }

    fn persist_history_fixture(
        worker_name: &str,
        session_id: SessionId,
        active_segment_id: SegmentId,
        segments: Vec<(SegmentId, Vec<LogEntry>)>,
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let aggregate_root = root.path().join(worker_name);
        let session = WorkerSessionStore::new(aggregate_root.join("session")).unwrap();
        for (segment_id, entries) in segments {
            session
                .create_segment(session_id, segment_id, &entries)
                .unwrap();
        }
        let aggregate = WorkerAggregateStore::new(&aggregate_root, worker_name).unwrap();
        aggregate
            .write(&crate::WorkerMetadata::new(
                worker_name,
                Some(crate::WorkerActiveSegmentRef::active_segment(
                    session_id,
                    active_segment_id,
                )),
            ))
            .unwrap();
        (root, aggregate_root)
    }

    fn linear_fixture(
        worker_name: &str,
        turn_count: usize,
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        let session_id = crate::new_session_id();
        let segment_id = crate::new_segment_id();
        let mut log = vec![segment_start(session_id, Vec::new(), None, None)];
        for turn in 1..=turn_count {
            append_turn(&mut log, turn);
        }
        persist_history_fixture(worker_name, session_id, segment_id, vec![(segment_id, log)])
    }

    fn turn_ids(page: &SessionHistoryPage) -> Vec<&str> {
        page.turns
            .iter()
            .map(|turn| turn.turn_id.as_str())
            .collect()
    }

    #[test]
    fn retained_history_pages_twelve_turns_as_five_five_two_across_compaction() {
        let worker_name = "worker-history-12";
        let session_id = crate::new_session_id();
        let source_segment = crate::new_segment_id();
        let middle_segment = crate::new_segment_id();
        let active_segment = crate::new_segment_id();
        let mut source_log = vec![segment_start(session_id, Vec::new(), None, None)];
        for turn in 1..=5 {
            append_turn(&mut source_log, turn);
        }
        let mut middle_log = vec![segment_start(
            session_id,
            vec![user_message(5), assistant_message(5, "final")],
            None,
            Some(SegmentOrigin {
                segment_id: source_segment,
                at_turn_index: 5,
            }),
        )];
        for turn in 6..=10 {
            append_turn(&mut middle_log, turn);
        }
        let synthetic_compaction_input = history_message(
            "derived-user",
            LoggedRole::User,
            "synthetic compaction input",
            LoggedSessionHistoryOrigin::DerivedSummary,
        );
        let mut active_log = vec![segment_start(
            session_id,
            vec![
                user_message(10),
                assistant_message(10, "final"),
                synthetic_compaction_input,
            ],
            None,
            Some(SegmentOrigin {
                segment_id: middle_segment,
                at_turn_index: 10,
            }),
        )];
        append_turn(&mut active_log, 11);
        append_turn(&mut active_log, 12);
        let (_root, aggregate_root) = persist_history_fixture(
            worker_name,
            session_id,
            active_segment,
            vec![
                (source_segment, source_log),
                (middle_segment, middle_log),
                (active_segment, active_log),
            ],
        );

        let newest = read_retained_session_history_page(
            &aggregate_root,
            worker_name,
            None,
            None,
            RetainedHistoryReadLimits::default(),
        )
        .unwrap();
        assert_eq!(
            turn_ids(&newest),
            vec!["user-8", "user-9", "user-10", "user-11", "user-12"]
        );
        assert!(newest.has_more);
        assert!(newest.next_cursor.is_some());
        assert_eq!(
            newest.turns[2]
                .entries
                .iter()
                .filter(|entry| entry.entry_id == "user-10")
                .count(),
            1,
            "the active Segment seed must not duplicate the source turn"
        );
        assert!(newest.turns.iter().all(|turn| {
            turn.entries
                .iter()
                .all(|entry| entry.provenance != SessionEntryProvenance::DerivedSummary)
        }));

        let middle = read_retained_session_history_page(
            &aggregate_root,
            worker_name,
            newest.next_cursor.as_deref(),
            None,
            RetainedHistoryReadLimits::default(),
        )
        .unwrap();
        assert_eq!(
            turn_ids(&middle),
            vec!["user-3", "user-4", "user-5", "user-6", "user-7"]
        );
        assert!(middle.has_more);

        let oldest = read_retained_session_history_page(
            &aggregate_root,
            worker_name,
            middle.next_cursor.as_deref(),
            None,
            RetainedHistoryReadLimits::default(),
        )
        .unwrap();
        assert_eq!(turn_ids(&oldest), vec!["user-1", "user-2"]);
        assert!(!oldest.has_more);
        assert!(oldest.next_cursor.is_none());
    }

    #[test]
    fn retained_history_page_boundaries_cover_zero_one_four_five_and_six_turns() {
        for count in [0, 1, 4, 5, 6] {
            let worker_name = format!("worker-history-{count}");
            let (_root, aggregate_root) = linear_fixture(&worker_name, count);
            let page = read_retained_session_history_page(
                &aggregate_root,
                &worker_name,
                None,
                None,
                RetainedHistoryReadLimits::default(),
            )
            .unwrap();
            assert_eq!(page.turns.len(), count.min(5), "turn count {count}");
            assert_eq!(page.has_more, count > 5, "turn count {count}");
            assert_eq!(page.next_cursor.is_some(), count > 5, "turn count {count}");
            if count == 6 {
                let prior = read_retained_session_history_page(
                    &aggregate_root,
                    &worker_name,
                    page.next_cursor.as_deref(),
                    None,
                    RetainedHistoryReadLimits::default(),
                )
                .unwrap();
                assert_eq!(turn_ids(&prior), vec!["user-1"]);
                assert!(!prior.has_more);
            }
        }
    }

    #[test]
    fn retained_history_keeps_cross_compaction_response_in_one_real_user_turn() {
        let worker_name = "worker-cross-compact";
        let session_id = crate::new_session_id();
        let source_segment = crate::new_segment_id();
        let active_segment = crate::new_segment_id();
        let mut source_log = vec![segment_start(session_id, Vec::new(), None, None)];
        source_log.push(LogEntry::Invoke {
            ts: 10,
            trigger: InvokeKind::UserSend,
        });
        source_log.push(LogEntry::AnnotatedUserInput {
            ts: 11,
            segments: vec![Segment::Text {
                content: "request 1".into(),
            }],
            history: vec![user_message(1)],
            extensions: Vec::new(),
        });
        source_log.push(LogEntry::AnnotatedAssistantItem {
            ts: 12,
            entry: assistant_message(1, "accepted"),
        });
        source_log.push(LogEntry::AnnotatedAssistantItem {
            ts: 13,
            entry: assistant_message(1, "progress"),
        });
        source_log.push(LogEntry::TurnEnd {
            ts: 14,
            turn_count: 1,
        });
        let mut active_log = vec![segment_start(
            session_id,
            vec![history_message(
                "derived-user",
                LoggedRole::User,
                "summarize prior context",
                LoggedSessionHistoryOrigin::DerivedSummary,
            )],
            None,
            Some(SegmentOrigin {
                segment_id: source_segment,
                at_turn_index: 1,
            }),
        )];
        active_log.push(LogEntry::AnnotatedAssistantItem {
            ts: 20,
            entry: assistant_message(1, "completed"),
        });
        let (_root, aggregate_root) = persist_history_fixture(
            worker_name,
            session_id,
            active_segment,
            vec![(source_segment, source_log), (active_segment, active_log)],
        );

        let page = read_retained_session_history_page(
            &aggregate_root,
            worker_name,
            None,
            None,
            RetainedHistoryReadLimits::default(),
        )
        .unwrap();

        assert_eq!(page.turns.len(), 1);
        assert_eq!(page.turns[0].turn_id, "user-1");
        assert_eq!(
            page.turns[0]
                .entries
                .iter()
                .map(|entry| entry.entry_id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "user-1",
                "assistant-1-accepted",
                "assistant-1-progress",
                "assistant-1-completed"
            ]
        );
    }

    #[test]
    fn retained_history_deduplicates_seed_history_by_stable_entry_id() {
        let worker_name = "worker-duplicate-seed";
        let session_id = crate::new_session_id();
        let source_segment = crate::new_segment_id();
        let active_segment = crate::new_segment_id();
        let mut source_log = vec![segment_start(session_id, Vec::new(), None, None)];
        append_turn(&mut source_log, 1);
        let active_log = vec![segment_start(
            session_id,
            vec![user_message(1), assistant_message(1, "final")],
            None,
            Some(SegmentOrigin {
                segment_id: source_segment,
                at_turn_index: 1,
            }),
        )];
        let (_root, aggregate_root) = persist_history_fixture(
            worker_name,
            session_id,
            active_segment,
            vec![(source_segment, source_log), (active_segment, active_log)],
        );

        let page = read_retained_session_history_page(
            &aggregate_root,
            worker_name,
            None,
            None,
            RetainedHistoryReadLimits::default(),
        )
        .unwrap();
        assert_eq!(page.turns.len(), 1);
        assert_eq!(page.turns[0].entries.len(), 2);
        assert_eq!(turn_ids(&page), vec!["user-1"]);
    }

    #[test]
    fn retained_history_fork_uses_persisted_turn_boundary_not_ui_turn_count() {
        let worker_name = "worker-fork-boundary";
        let session_id = crate::new_session_id();
        let source_segment = crate::new_segment_id();
        let active_segment = crate::new_segment_id();
        let mut source_log = vec![segment_start(session_id, Vec::new(), None, None)];
        source_log.push(LogEntry::Invoke {
            ts: 10,
            trigger: InvokeKind::UserSend,
        });
        source_log.push(LogEntry::AnnotatedUserInput {
            ts: 11,
            segments: vec![Segment::Text {
                content: "request 1".into(),
            }],
            history: vec![user_message(1)],
            extensions: Vec::new(),
        });
        source_log.push(LogEntry::AnnotatedAssistantItem {
            ts: 12,
            entry: assistant_message(1, "boundary"),
        });
        source_log.push(LogEntry::TurnEnd {
            ts: 13,
            turn_count: 1,
        });
        source_log.push(LogEntry::Invoke {
            ts: 14,
            trigger: InvokeKind::Notify,
        });
        source_log.push(LogEntry::AnnotatedAssistantItem {
            ts: 15,
            entry: assistant_message(1, "unadopted"),
        });
        source_log.push(LogEntry::TurnEnd {
            ts: 16,
            turn_count: 2,
        });
        append_turn(&mut source_log, 2);

        let mut active_log = vec![segment_start(
            session_id,
            vec![
                user_message(1),
                assistant_message(1, "boundary"),
                user_message(2),
                assistant_message(2, "final"),
            ],
            Some(SegmentOrigin {
                segment_id: source_segment,
                at_turn_index: 1,
            }),
            None,
        )];
        append_turn(&mut active_log, 3);
        let (_root, aggregate_root) = persist_history_fixture(
            worker_name,
            session_id,
            active_segment,
            vec![(source_segment, source_log), (active_segment, active_log)],
        );

        let page = read_retained_session_history_page(
            &aggregate_root,
            worker_name,
            None,
            None,
            RetainedHistoryReadLimits::default(),
        )
        .unwrap();

        assert_eq!(turn_ids(&page), vec!["user-1", "user-3"]);
        assert!(page.turns.iter().all(|turn| {
            turn.entries
                .iter()
                .all(|entry| entry.entry_id != "assistant-1-unadopted")
        }));
        assert!(page.turns.iter().all(|turn| turn.turn_id != "user-2"));
    }

    #[test]
    fn retained_history_rejects_a_lineage_segment_from_another_session() {
        let worker_name = "worker-foreign-session";
        let session_id = crate::new_session_id();
        let foreign_session_id = crate::new_session_id();
        let source_segment = crate::new_segment_id();
        let active_segment = crate::new_segment_id();
        let source_log = vec![segment_start(foreign_session_id, Vec::new(), None, None)];
        let active_log = vec![segment_start(
            session_id,
            Vec::new(),
            Some(SegmentOrigin {
                segment_id: source_segment,
                at_turn_index: 0,
            }),
            None,
        )];
        let (_root, aggregate_root) = persist_history_fixture(
            worker_name,
            session_id,
            active_segment,
            vec![(source_segment, source_log), (active_segment, active_log)],
        );

        let error = read_retained_session_history_page(
            &aggregate_root,
            worker_name,
            None,
            None,
            RetainedHistoryReadLimits::default(),
        )
        .unwrap_err();

        assert_eq!(error, RetainedHistoryReadError::CorruptLog);
    }

    #[test]
    fn retained_history_cursor_rejects_invalid_cross_worker_and_stale_lineage() {
        let (_root_a, aggregate_a) = linear_fixture("worker-a", 6);
        let page = read_retained_session_history_page(
            &aggregate_a,
            "worker-a",
            None,
            None,
            RetainedHistoryReadLimits::default(),
        )
        .unwrap();
        let cursor = page.next_cursor.unwrap();

        let (_root_b, aggregate_b) = linear_fixture("worker-b", 6);
        assert_eq!(
            read_retained_session_history_page(
                &aggregate_b,
                "worker-b",
                Some(&cursor),
                None,
                RetainedHistoryReadLimits::default(),
            )
            .unwrap_err(),
            RetainedHistoryReadError::InvalidCursor
        );
        assert_eq!(
            read_retained_session_history_page(
                &aggregate_a,
                "worker-a",
                Some("not-base64!"),
                None,
                RetainedHistoryReadLimits::default(),
            )
            .unwrap_err(),
            RetainedHistoryReadError::InvalidCursor
        );

        let aggregate = WorkerAggregateStore::new(&aggregate_a, "worker-a").unwrap();
        let metadata = aggregate.read_by_name("worker-a").unwrap().unwrap();
        let active = metadata.active.unwrap();
        let old_segment = active.segment_id.unwrap();
        let new_segment = crate::new_segment_id();
        WorkerSessionStore::new(aggregate_a.join("session"))
            .unwrap()
            .create_segment(
                active.session_id,
                new_segment,
                &[segment_start(
                    active.session_id,
                    Vec::new(),
                    None,
                    Some(SegmentOrigin {
                        segment_id: old_segment,
                        at_turn_index: 6,
                    }),
                )],
            )
            .unwrap();
        aggregate
            .write(&crate::WorkerMetadata::new(
                "worker-a",
                Some(crate::WorkerActiveSegmentRef::active_segment(
                    active.session_id,
                    new_segment,
                )),
            ))
            .unwrap();
        assert_eq!(
            read_retained_session_history_page(
                &aggregate_a,
                "worker-a",
                Some(&cursor),
                None,
                RetainedHistoryReadLimits::default(),
            )
            .unwrap_err(),
            RetainedHistoryReadError::InvalidCursor
        );
    }

    #[test]
    fn retained_history_enforces_page_scan_and_response_limits_without_splitting_turns() {
        let worker_name = "worker-history-limits";
        let (_root, aggregate_root) = linear_fixture(worker_name, 1);
        let defaults = RetainedHistoryReadLimits::default();

        for limits in [
            RetainedHistoryReadLimits {
                max_scan_bytes: 0,
                ..defaults
            },
            RetainedHistoryReadLimits {
                max_segments: 0,
                ..defaults
            },
            RetainedHistoryReadLimits {
                max_entries: 1,
                ..defaults
            },
            RetainedHistoryReadLimits {
                max_response_bytes: 1,
                ..defaults
            },
        ] {
            assert_eq!(
                read_retained_session_history_page(
                    &aggregate_root,
                    worker_name,
                    None,
                    None,
                    limits,
                )
                .unwrap_err(),
                RetainedHistoryReadError::ResourceLimit
            );
        }
        assert_eq!(
            read_retained_session_history_page(
                &aggregate_root,
                worker_name,
                None,
                Some(MAX_RETAINED_HISTORY_PAGE_TURNS + 1),
                defaults,
            )
            .unwrap_err(),
            RetainedHistoryReadError::ResourceLimit
        );

        let page = read_retained_session_history_page(
            &aggregate_root,
            worker_name,
            None,
            Some(1),
            defaults,
        )
        .unwrap();
        assert_eq!(page.turns.len(), 1);
        assert_eq!(page.turns[0].entries.len(), 2);
    }

    #[test]
    fn retained_history_rejects_an_oversized_single_turn_instead_of_splitting_it() {
        let worker_name = "worker-oversized-turn";
        let session_id = crate::new_session_id();
        let segment_id = crate::new_segment_id();
        let body = "x".repeat(4096);
        let log = vec![
            segment_start(session_id, Vec::new(), None, None),
            LogEntry::Invoke {
                ts: 2,
                trigger: InvokeKind::UserSend,
            },
            LogEntry::AnnotatedUserInput {
                ts: 3,
                segments: vec![Segment::Text {
                    content: body.clone(),
                }],
                history: vec![history_message(
                    "large-user",
                    LoggedRole::User,
                    body,
                    LoggedSessionHistoryOrigin::HumanInput {
                        account_id: "account-1".into(),
                    },
                )],
                extensions: Vec::new(),
            },
            LogEntry::AnnotatedAssistantItem {
                ts: 4,
                entry: history_message(
                    "large-assistant",
                    LoggedRole::Assistant,
                    "complete",
                    LoggedSessionHistoryOrigin::LegacyUnknown,
                ),
            },
        ];
        let (_root, aggregate_root) =
            persist_history_fixture(worker_name, session_id, segment_id, vec![(segment_id, log)]);

        let error = read_retained_session_history_page(
            &aggregate_root,
            worker_name,
            None,
            Some(1),
            RetainedHistoryReadLimits {
                max_response_bytes: 1024,
                ..RetainedHistoryReadLimits::default()
            },
        )
        .unwrap_err();

        assert_eq!(error, RetainedHistoryReadError::ResourceLimit);
    }

    #[test]
    fn current_projection_is_stable_and_hides_reasoning_and_system_prompts() {
        let session_id = crate::new_session_id();
        let log = vec![LogEntry::AnnotatedSegmentStart {
            ts: 1,
            session_id,
            system_prompt: None,
            config: RequestConfig::default(),
            history: vec![
                LoggedItem::Message {
                    role: LoggedRole::System,
                    content: vec![LoggedContentPart::Text {
                        text: "secret prompt".into(),
                    }],
                },
                LoggedItem::Reasoning {
                    text: "secret reasoning".into(),
                    summary: Vec::new(),
                    encrypted_content: None,
                    signature: None,
                },
                LoggedItem::Message {
                    role: LoggedRole::Assistant,
                    content: vec![LoggedContentPart::Text {
                        text: "visible".into(),
                    }],
                },
            ]
            .into_iter()
            .map(|item| LoggedHistoryEntry {
                item,
                metadata: LoggedSessionHistoryMetadata {
                    entry_id: LoggedSessionHistoryEntryId::new(),
                    origin: LoggedSessionHistoryOrigin::LegacyUnknown,
                    derivation: None,
                },
            })
            .collect(),
            forked_from: None,
            compacted_from: None,
        }];

        let first = project_session_snapshot(session_id, &log);
        let second = project_session_snapshot(session_id, &log);
        assert_eq!(first, second);
        assert_eq!(first.entries.len(), 1);
        assert_eq!(first.entries[0].timestamp, 1);
        assert_eq!(
            first.entries[0].provenance,
            SessionEntryProvenance::LegacyUnknown
        );
        let json = serde_json::to_string(&first).unwrap();
        assert!(!json.contains("secret prompt"));
        assert!(!json.contains("secret reasoning"));
        assert!(json.contains("visible"));
    }

    #[test]
    fn compacted_checkpoint_restores_uploaded_file_segments() {
        let session_id = crate::new_session_id();
        let user_entry_id = LoggedSessionHistoryEntryId::new();
        let file = protocol::UploadedFileRef {
            artifact_id: "019ca7c8-57b6-7f05-8edf-524147aba7b3".into(),
            file_name: "notes.md".into(),
            media_type: "text/markdown".into(),
            created_at_ms: 7,
            availability: protocol::UploadedFileAvailability::Available,
            byte_len: 12,
            sha256: "a".repeat(64),
            source_entry_id: Some(user_entry_id.0.clone()),
        };
        let segment = Segment::UploadedFile { file };
        let log = vec![
            LogEntry::AnnotatedSegmentStart {
                ts: 10,
                session_id,
                system_prompt: None,
                config: RequestConfig::default(),
                history: vec![LoggedHistoryEntry {
                    item: LoggedItem::Message {
                        role: LoggedRole::User,
                        content: vec![LoggedContentPart::Text {
                            text: "[Attached file: notes.md]".into(),
                        }],
                    },
                    metadata: LoggedSessionHistoryMetadata {
                        entry_id: user_entry_id,
                        origin: LoggedSessionHistoryOrigin::HumanInput {
                            account_id: "account-1".into(),
                        },
                        derivation: None,
                    },
                }],
                forked_from: None,
                compacted_from: Some(crate::SegmentOrigin {
                    segment_id: crate::new_segment_id(),
                    at_turn_index: 1,
                }),
            },
            LogEntry::InputSegmentsCheckpoint {
                ts: 10,
                user_segments: vec![vec![segment.clone()]],
            },
        ];

        let snapshot = project_current_session_snapshot(&log);
        assert_eq!(
            snapshot.entries[0].data,
            SessionSnapshotEntryData::UserInput {
                segments: vec![segment]
            }
        );
    }

    #[test]
    fn annotated_user_input_attaches_segments_to_first_user_role_entry_for_any_origin() {
        let session_id = crate::new_session_id();
        let segments = vec![Segment::Text {
            content: "normal submit".into(),
        }];

        for origin in [
            LoggedSessionHistoryOrigin::LegacyUnknown,
            LoggedSessionHistoryOrigin::FlowInstruction {
                selector: "builtin:coder-review".into(),
                definition_id: "flow-definition".into(),
                definition_revision: 7,
                instance_id: "flow-instance".into(),
                state_id: "implement".into(),
            },
        ] {
            let user_entry_id = LoggedSessionHistoryEntryId::new();
            let source_entry_id = LoggedSessionHistoryEntryId::new();
            let log = vec![
                LogEntry::AnnotatedSegmentStart {
                    ts: 1,
                    session_id,
                    system_prompt: None,
                    config: RequestConfig::default(),
                    history: Vec::new(),
                    forked_from: None,
                    compacted_from: None,
                },
                LogEntry::AnnotatedUserInput {
                    ts: 2,
                    segments: segments.clone(),
                    history: vec![
                        LoggedHistoryEntry {
                            item: LoggedItem::Message {
                                role: LoggedRole::System,
                                content: vec![LoggedContentPart::Text {
                                    text: "flow instruction".into(),
                                }],
                            },
                            metadata: LoggedSessionHistoryMetadata {
                                entry_id: LoggedSessionHistoryEntryId::new(),
                                origin: LoggedSessionHistoryOrigin::FlowInstruction {
                                    selector: "builtin:coder-review".into(),
                                    definition_id: "flow-definition".into(),
                                    definition_revision: 7,
                                    instance_id: "flow-instance".into(),
                                    state_id: "implement".into(),
                                },
                                derivation: None,
                            },
                        },
                        LoggedHistoryEntry {
                            item: LoggedItem::Message {
                                role: LoggedRole::User,
                                content: vec![LoggedContentPart::Text {
                                    text: "normal submit".into(),
                                }],
                            },
                            metadata: LoggedSessionHistoryMetadata {
                                entry_id: user_entry_id.clone(),
                                origin: origin.clone(),
                                derivation: Some(LoggedHistoryDerivation {
                                    sources: vec![source_entry_id.clone()],
                                }),
                            },
                        },
                    ],
                    extensions: Vec::new(),
                },
            ];

            let snapshot = project_current_session_snapshot(&log);
            assert_eq!(snapshot.entries.len(), 1);
            assert_eq!(snapshot.entries[0].entry_id, user_entry_id.0);
            assert_eq!(snapshot.entries[0].provenance, provenance(&origin));
            assert_eq!(snapshot.entries[0].derived_from, vec![source_entry_id.0]);
            assert_eq!(
                snapshot.entries[0].data,
                SessionSnapshotEntryData::UserInput {
                    segments: segments.clone(),
                }
            );
        }
    }

    #[test]
    fn annotated_projection_preserves_identity_and_provenance() {
        let session_id = crate::new_session_id();
        let metadata = LoggedSessionHistoryMetadata {
            entry_id: LoggedSessionHistoryEntryId::new(),
            origin: LoggedSessionHistoryOrigin::ModelOutput {
                worker: LoggedWorkerSubject {
                    workspace_id: None,
                    runtime_id: None,
                    worker_id: "worker".into(),
                },
            },
            derivation: None,
        };
        let expected_id = metadata.entry_id.0.clone();
        let log = vec![LogEntry::AnnotatedSegmentStart {
            ts: 1,
            session_id,
            system_prompt: None,
            config: RequestConfig::default(),
            history: vec![LoggedHistoryEntry {
                item: LoggedItem::Message {
                    role: LoggedRole::Assistant,
                    content: vec![LoggedContentPart::Text { text: "ok".into() }],
                },
                metadata,
            }],
            forked_from: None,
            compacted_from: None,
        }];

        let snapshot = project_session_snapshot(session_id, &log);
        assert_eq!(snapshot.entries[0].entry_id, expected_id);
        assert_eq!(
            snapshot.entries[0].provenance,
            SessionEntryProvenance::ModelOutput
        );
    }

    #[test]
    fn retained_snapshot_read_projects_active_segment_without_rewriting_files() {
        let root = tempfile::tempdir().unwrap();
        let aggregate_root = root.path().join("worker-a");
        let aggregate = WorkerAggregateStore::new(&aggregate_root, "worker-a").unwrap();
        let session_id = crate::new_session_id();
        let segment_id = crate::new_segment_id();
        aggregate
            .write(&crate::WorkerMetadata::new(
                "worker-a",
                Some(crate::WorkerActiveSegmentRef::active_segment(
                    session_id, segment_id,
                )),
            ))
            .unwrap();
        let session = WorkerSessionStore::new(aggregate_root.join("session")).unwrap();
        session.create_segment(session_id, segment_id, &[]).unwrap();
        let metadata_path = aggregate_root.join("metadata.json");
        let manifest_path = aggregate_root.join("session/session.json");
        let segment_path = aggregate_root
            .join("session/segments")
            .join(format!("{segment_id}.jsonl"));
        let metadata_before = std::fs::read(&metadata_path).unwrap();
        let manifest_before = std::fs::read(&manifest_path).unwrap();
        let old_atime = filetime::FileTime::from_unix_time(946_684_800, 0);
        for path in [&metadata_path, &manifest_path, &segment_path] {
            filetime::set_file_atime(path, old_atime).unwrap();
        }
        let mtimes_before = [&metadata_path, &manifest_path, &segment_path].map(|path| {
            filetime::FileTime::from_last_modification_time(&std::fs::metadata(path).unwrap())
        });

        let retained = read_retained_session_snapshot(
            &aggregate_root,
            "worker-a",
            DEFAULT_RETAINED_SNAPSHOT_MAX_BYTES,
        )
        .unwrap();
        let history = read_retained_session_history_page(
            &aggregate_root,
            "worker-a",
            None,
            None,
            RetainedHistoryReadLimits::default(),
        )
        .unwrap();

        assert_eq!(retained.identity.session_id, session_id.to_string());
        assert_eq!(retained.identity.segment_id, segment_id.to_string());
        assert_eq!(retained.identity.entry_count, 0);
        assert!(retained.snapshot.entries.is_empty());
        assert!(history.turns.is_empty());
        assert!(!history.has_more);
        for (index, path) in [&metadata_path, &manifest_path, &segment_path]
            .into_iter()
            .enumerate()
        {
            let metadata = std::fs::metadata(path).unwrap();
            assert_eq!(
                filetime::FileTime::from_last_access_time(&metadata),
                old_atime,
                "retained observation changed access time for {}",
                path.display()
            );
            assert_eq!(
                filetime::FileTime::from_last_modification_time(&metadata),
                mtimes_before[index],
                "retained observation changed modification time for {}",
                path.display()
            );
        }
        assert_eq!(std::fs::read(&metadata_path).unwrap(), metadata_before);
        assert_eq!(std::fs::read(&manifest_path).unwrap(), manifest_before);
        assert!(matches!(
            read_retained_session_snapshot(&aggregate_root, "worker-a", 0),
            Err(RetainedSnapshotReadError::SnapshotTooLarge)
        ));
    }

    #[test]
    fn retained_reads_do_not_create_missing_aggregate() {
        let root = tempfile::tempdir().unwrap();
        let aggregate = root.path().join("missing-worker");

        let error = read_retained_session_snapshot(
            &aggregate,
            "missing-worker",
            DEFAULT_RETAINED_SNAPSHOT_MAX_BYTES,
        )
        .unwrap_err();

        assert!(matches!(error, RetainedSnapshotReadError::RetentionMissing));
        assert_eq!(
            read_retained_session_history_page(
                &aggregate,
                "missing-worker",
                None,
                None,
                RetainedHistoryReadLimits::default(),
            )
            .unwrap_err(),
            RetainedHistoryReadError::RetentionMissing
        );
        assert!(!aggregate.exists());
    }
}
