use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use protocol::{
    Segment, SessionContentPart, SessionConversationTurn, SessionEntryProvenance,
    SessionHistoryLineageBoundary, SessionHistoryPage, SessionMessageRole, SessionSnapshot,
    SessionSnapshotEntry, SessionSnapshotEntryData, SessionToolAttachment,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use std::collections::HashSet;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedSessionAttachment {
    pub media_type: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RetainedAttachmentReadError {
    #[error("retained worker state is unavailable")]
    RetentionMissing,
    #[error("retained worker state has no active session pointer")]
    ActivePointerMissing,
    #[error("retained session identity does not match")]
    SessionMismatch,
    #[error("session attachment was not found")]
    NotFound,
    #[error("retained session requires migration")]
    MigrationRequired,
    #[error("retained session log is corrupt")]
    CorruptLog,
    #[error("retained session storage is unavailable")]
    StorageUnavailable,
    #[error("retained session attachment exceeds a resource limit")]
    ResourceLimit,
}

pub const DEFAULT_RETAINED_HISTORY_PAGE_TURNS: usize = 5;
pub const MAX_RETAINED_HISTORY_PAGE_TURNS: usize = 5;
pub const DEFAULT_RETAINED_HISTORY_MAX_SCAN_BYTES: u64 = 64 * 1024 * 1024;
pub const DEFAULT_RETAINED_HISTORY_MAX_SEGMENTS: usize = 64;
pub const DEFAULT_RETAINED_HISTORY_MAX_ENTRIES: usize = 100_000;
pub const DEFAULT_RETAINED_HISTORY_MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;

/// Explicit resource bounds for one retained-history page read.
///
/// The operation never truncates a turn to fit these limits. Bounded lineage
/// metadata or one selected page that exceeds a bound fails with
/// [`RetainedHistoryReadError::ResourceLimit`].
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
    active_segment_id: String,
    lineage_id: String,
    before_turn_id: String,
    segment_id: String,
    before_offset: u64,
    record_end_offset: Option<u64>,
    seed_entry_index: Option<usize>,
}

#[derive(Debug)]
struct LoadedLineageSegment {
    segment_id: SegmentId,
    adopted_through_turn: Option<usize>,
    file_len: u64,
    seed_end_offset: u64,
    origin: Option<LineageOrigin>,
    seed_entries: Vec<SessionSnapshotEntry>,
}

#[derive(Debug, Clone)]
struct HistoryTurnPosition {
    segment_id: SegmentId,
    before_offset: u64,
    record_end_offset: Option<u64>,
    seed_entry_index: Option<usize>,
}

#[derive(Debug)]
struct PositionedHistoryTurn {
    turn: SessionConversationTurn,
    position: HistoryTurnPosition,
}

#[derive(Debug, Clone, Copy)]
enum LineageOriginKind {
    Fork,
    Compact,
}

#[derive(Debug, Clone)]
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
    if store.segment_log_len(segment_id).map_err(map_store_error)?
        > DEFAULT_RETAINED_HISTORY_MAX_SCAN_BYTES
    {
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

/// Resolve one immutable image body from the active retained Session. The
/// caller supplies only public identities; storage paths never cross this API.
pub fn read_retained_session_attachment(
    aggregate_root: &Path,
    worker_name: &str,
    expected_session_id: &str,
    attachment_id: &str,
    max_scan_bytes: u64,
    max_attachment_bytes: u64,
) -> Result<RetainedSessionAttachment, RetainedAttachmentReadError> {
    let aggregate = WorkerAggregateStore::open_read_only(aggregate_root, worker_name)
        .map_err(map_attachment_worker_store_error)?;
    let metadata = aggregate
        .read_read_only()
        .map_err(map_attachment_worker_store_error)?
        .ok_or(RetainedAttachmentReadError::RetentionMissing)?;
    let active = metadata
        .active
        .ok_or(RetainedAttachmentReadError::ActivePointerMissing)?;
    if active.session_id.to_string() != expected_session_id {
        return Err(RetainedAttachmentReadError::SessionMismatch);
    }
    let segment_id = active
        .segment_id
        .ok_or(RetainedAttachmentReadError::ActivePointerMissing)?;
    let store = WorkerSessionStore::open_read_only(aggregate_root.join("session"))
        .map_err(map_attachment_store_error)?;
    let limits = RetainedHistoryReadLimits {
        max_scan_bytes,
        ..RetainedHistoryReadLimits::default()
    };
    let (lineage, _, mut scanned_bytes, mut scanned_entries) =
        load_adopted_lineage(&store, active.session_id, segment_id, limits)
            .map_err(map_history_attachment_error)?;
    for segment in &lineage {
        let adopted_end = locate_adopted_segment_end(
            &store,
            active.session_id,
            segment,
            limits,
            &mut scanned_bytes,
            &mut scanned_entries,
        )
        .map_err(map_history_attachment_error)?;
        let mut reader = store
            .open_retained_segment_reader(active.session_id, segment.segment_id, adopted_end)
            .map_err(map_attachment_store_error)?;
        loop {
            let remaining = limits.max_scan_bytes.saturating_sub(scanned_bytes);
            let (record, bytes) = reader
                .previous_record(remaining)
                .map_err(map_attachment_store_error)?;
            scanned_bytes = scanned_bytes
                .checked_add(bytes)
                .ok_or(RetainedAttachmentReadError::ResourceLimit)?;
            let Some(record) = record else {
                break;
            };
            scanned_entries = scanned_entries
                .checked_add(persisted_entry_units(std::slice::from_ref(&record.entry)))
                .ok_or(RetainedAttachmentReadError::ResourceLimit)?;
            if scanned_entries > limits.max_entries {
                return Err(RetainedAttachmentReadError::ResourceLimit);
            }
            if let Some(result) =
                session_attachment_from_record(&record.entry, attachment_id, max_attachment_bytes)
            {
                return result;
            }
        }
    }
    Err(RetainedAttachmentReadError::NotFound)
}

pub fn read_session_attachment_from_entries(
    entries: &[LogEntry],
    attachment_id: &str,
    max_attachment_bytes: u64,
) -> Result<RetainedSessionAttachment, RetainedAttachmentReadError> {
    for record in entries {
        if let Some(result) =
            session_attachment_from_record(record, attachment_id, max_attachment_bytes)
        {
            return result;
        }
    }
    Err(RetainedAttachmentReadError::NotFound)
}

fn session_attachment_from_record(
    record: &LogEntry,
    attachment_id: &str,
    max_attachment_bytes: u64,
) -> Option<Result<RetainedSessionAttachment, RetainedAttachmentReadError>> {
    let mut found = None;
    visit_record_history(record, &mut |entry| {
        if found.is_none() {
            found = attachment_from_entry(entry, attachment_id, max_attachment_bytes);
        }
    });
    found
}

fn visit_record_history(record: &LogEntry, visit: &mut impl FnMut(&LoggedHistoryEntry)) {
    match record {
        LogEntry::AnnotatedSegmentStart { history, .. }
        | LogEntry::AnnotatedUserInput { history, .. } => {
            for entry in history {
                visit(entry);
            }
        }
        LogEntry::AnnotatedAssistantItem { entry, .. }
        | LogEntry::AnnotatedToolResult { entry, .. } => visit(entry),
        _ => {}
    }
}

fn attachment_from_entry(
    entry: &LoggedHistoryEntry,
    requested_id: &str,
    max_attachment_bytes: u64,
) -> Option<Result<RetainedSessionAttachment, RetainedAttachmentReadError>> {
    let LoggedItem::ToolResult { attachments, .. } = &entry.item else {
        return None;
    };
    for (attachment_index, attachment) in attachments.iter().enumerate() {
        let crate::logged_item::LoggedAttachment::Image { mime_type, data } = attachment;
        if session_tool_attachment_id(
            &entry.metadata.entry_id.0,
            attachment_index,
            mime_type,
            data,
        ) != requested_id
        {
            continue;
        }
        if data.len() as u64 > max_attachment_bytes {
            return Some(Err(RetainedAttachmentReadError::ResourceLimit));
        }
        return Some(Ok(RetainedSessionAttachment {
            media_type: mime_type.clone(),
            data: data.clone(),
        }));
    }
    None
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
    let (lineage, lineage_id, metadata_bytes, metadata_entries) =
        load_adopted_lineage(&store, active.session_id, active_segment_id, limits)?;
    let cursor = cursor.map(decode_history_cursor).transpose()?;
    let start = if let Some(cursor) = cursor.as_ref() {
        if cursor.version != 2
            || cursor.worker_name != worker_name
            || cursor.session_id != active.session_id.to_string()
            || cursor.active_segment_id != active_segment_id.to_string()
            || cursor.lineage_id != lineage_id
            || !lineage
                .iter()
                .any(|segment| segment.segment_id.to_string() == cursor.segment_id)
        {
            return Err(RetainedHistoryReadError::InvalidCursor);
        }
        let segment_id = cursor
            .segment_id
            .parse()
            .map_err(|_| RetainedHistoryReadError::InvalidCursor)?;
        Some((
            HistoryTurnPosition {
                segment_id,
                before_offset: cursor.before_offset,
                record_end_offset: cursor.record_end_offset,
                seed_entry_index: cursor.seed_entry_index,
            },
            cursor.before_turn_id.as_str(),
        ))
    } else {
        None
    };
    let mut scanned_bytes = metadata_bytes;
    let mut scanned_entries = metadata_entries;
    let parent_lineage = if cursor.is_none() && lineage.len() > 1 {
        let parent_turns = read_history_turns_backward(
            &store,
            active.session_id,
            &lineage[1..],
            None,
            1,
            limits,
            &mut scanned_bytes,
            &mut scanned_entries,
        )?;
        Some(SessionHistoryLineageBoundary {
            lineage_id: lineage_identity(active.session_id, &lineage[1..]),
            adopted_through_turn: parent_turns
                .into_iter()
                .next()
                .map(|positioned| positioned.turn),
        })
    } else {
        None
    };
    let mut turns = read_history_turns_backward(
        &store,
        active.session_id,
        &lineage,
        start,
        page_limit.saturating_add(1),
        limits,
        &mut scanned_bytes,
        &mut scanned_entries,
    )?;
    let has_more = turns.len() > page_limit;
    if has_more {
        turns.truncate(page_limit);
    }
    let next_cursor = if has_more {
        let oldest = turns
            .last()
            .expect("a page with earlier history cannot be empty");
        Some(encode_history_cursor(&RetainedHistoryCursor {
            version: 2,
            worker_name: worker_name.to_owned(),
            session_id: active.session_id.to_string(),
            active_segment_id: active_segment_id.to_string(),
            lineage_id: lineage_id.clone(),
            before_turn_id: oldest.turn.turn_id.clone(),
            segment_id: oldest.position.segment_id.to_string(),
            before_offset: oldest.position.before_offset,
            record_end_offset: oldest.position.record_end_offset,
            seed_entry_index: oldest.position.seed_entry_index,
        })?)
    } else {
        None
    };
    turns.reverse();
    let compact_ancestor_lineage_ids = compact_ancestor_lineage_ids(active.session_id, &lineage);
    let page = SessionHistoryPage {
        session_id: active.session_id.to_string(),
        lineage_id,
        compact_ancestor_lineage_ids,
        parent_lineage,
        turns: turns
            .into_iter()
            .map(|positioned| positioned.turn)
            .collect(),
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
) -> Result<(Vec<LoadedLineageSegment>, String, u64, usize), RetainedHistoryReadError> {
    let mut lineage = Vec::new();
    let mut visited = HashSet::new();
    let mut segment_id = active_segment_id;
    let mut adopted_through_turn = None;
    let mut scanned_bytes = 0_u64;
    let mut scanned_entries = 0_usize;

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
        let (first, segment_bytes, file_len) = store
            .read_first_log_record_read_only_bounded(session_id, segment_id, remaining_scan_bytes)
            .map_err(map_history_store_error)?;
        scanned_bytes = scanned_bytes
            .checked_add(segment_bytes)
            .ok_or(RetainedHistoryReadError::ResourceLimit)?;
        if let Some(first) = first.as_ref() {
            scanned_entries = scanned_entries
                .checked_add(persisted_entry_units(std::slice::from_ref(&first.entry)))
                .ok_or(RetainedHistoryReadError::ResourceLimit)?;
            if scanned_entries > limits.max_entries {
                return Err(RetainedHistoryReadError::ResourceLimit);
            }
        }
        let origin =
            validate_segment_start_entry(session_id, first.as_ref().map(|entry| &entry.entry))?;

        let seed_entries = if origin.is_none() {
            let mut seed_records = first
                .as_ref()
                .map(|first| vec![first.entry.clone()])
                .unwrap_or_default();
            if let Some(first) = first.as_ref() {
                let remaining_scan_bytes = limits.max_scan_bytes.saturating_sub(scanned_bytes);
                let (next, next_bytes) = store
                    .read_next_log_record_read_only_bounded(
                        session_id,
                        segment_id,
                        first.end_offset,
                        remaining_scan_bytes,
                    )
                    .map_err(map_history_store_error)?;
                scanned_bytes = scanned_bytes
                    .checked_add(next_bytes)
                    .ok_or(RetainedHistoryReadError::ResourceLimit)?;
                if let Some(next) = next {
                    scanned_entries = scanned_entries
                        .checked_add(persisted_entry_units(std::slice::from_ref(&next.entry)))
                        .ok_or(RetainedHistoryReadError::ResourceLimit)?;
                    if scanned_entries > limits.max_entries {
                        return Err(RetainedHistoryReadError::ResourceLimit);
                    }
                    if matches!(&next.entry, LogEntry::InputSegmentsCheckpoint { .. }) {
                        seed_records.push(next.entry);
                    }
                }
            }
            project_session_snapshot_for_segment(session_id, Some(segment_id), &seed_records)
                .entries
                .into_iter()
                .filter(|entry| entry.provenance != SessionEntryProvenance::DerivedSummary)
                .collect()
        } else {
            Vec::new()
        };
        lineage.push(LoadedLineageSegment {
            segment_id,
            adopted_through_turn,
            file_len,
            seed_end_offset: first.as_ref().map(|record| record.end_offset).unwrap_or(0),
            origin: origin.clone(),
            seed_entries,
        });
        let Some(origin) = origin else {
            break;
        };
        segment_id = origin.origin.segment_id;
        adopted_through_turn = Some(origin.origin.at_turn_index);
    }

    let lineage_id = lineage_identity(session_id, &lineage);
    Ok((lineage, lineage_id, scanned_bytes, scanned_entries))
}

fn lineage_identity(session_id: SessionId, lineage: &[LoadedLineageSegment]) -> String {
    let active_segment_id = lineage
        .first()
        .expect("an adopted lineage always contains its active Segment")
        .segment_id;
    let mut hash = Sha256::new();
    hash.update(b"yoi-retained-history-lineage-v2\0");
    hash.update(session_id.as_bytes());
    hash.update(active_segment_id.as_bytes());
    for (index, segment) in lineage.iter().enumerate() {
        hash.update(segment.segment_id.as_bytes());
        let boundary = (index != 0)
            .then_some(segment.adopted_through_turn)
            .flatten();
        match boundary {
            Some(turn) => {
                hash.update([1]);
                hash.update((turn as u64).to_be_bytes());
            }
            None => hash.update([0]),
        }
        if let Some(origin) = &segment.origin {
            hash.update(match origin.kind {
                LineageOriginKind::Fork => [1],
                LineageOriginKind::Compact => [2],
            });
            hash.update(origin.origin.segment_id.as_bytes());
            hash.update((origin.origin.at_turn_index as u64).to_be_bytes());
        } else {
            hash.update([0]);
        }
    }
    URL_SAFE_NO_PAD.encode(hash.finalize())
}

fn compact_ancestor_lineage_ids(
    session_id: SessionId,
    lineage: &[LoadedLineageSegment],
) -> Vec<String> {
    let mut ancestors = Vec::new();
    for index in 0..lineage.len().saturating_sub(1) {
        if !matches!(
            lineage[index].origin.as_ref().map(|origin| origin.kind),
            Some(LineageOriginKind::Compact)
        ) {
            break;
        }
        ancestors.push(lineage_identity(session_id, &lineage[index + 1..]));
    }
    ancestors
}

fn validate_segment_start_entry(
    expected_session_id: SessionId,
    entry: Option<&LogEntry>,
) -> Result<Option<LineageOrigin>, RetainedHistoryReadError> {
    let Some(entry) = entry else {
        return Ok(None);
    };
    let LogEntry::AnnotatedSegmentStart {
        session_id,
        forked_from,
        compacted_from,
        ..
    } = entry
    else {
        return Err(RetainedHistoryReadError::CorruptLog);
    };
    if *session_id != expected_session_id {
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

fn locate_adopted_segment_end(
    store: &WorkerSessionStore,
    session_id: SessionId,
    segment: &LoadedLineageSegment,
    limits: RetainedHistoryReadLimits,
    scanned_bytes: &mut u64,
    scanned_entries: &mut usize,
) -> Result<u64, RetainedHistoryReadError> {
    const FORWARD_BOUNDARY_TURN_THRESHOLD: usize = 64;

    let Some(boundary) = segment.adopted_through_turn else {
        return Ok(segment.file_len);
    };
    let inherited_turns = segment
        .origin
        .as_ref()
        .map(|origin| origin.origin.at_turn_index)
        .unwrap_or(0);
    if boundary <= inherited_turns {
        // This edge adopts no locally completed turns. The Segment's own
        // origin remains an independent edge with its original boundary.
        return Ok(segment.seed_end_offset);
    }

    // Small/early boundaries are cheapest to resolve from the Segment seed and
    // must not pay for an arbitrarily large unadopted suffix. Later boundaries
    // are resolved backward so a large adopted prefix does not have to be read
    // merely to find the nearby split point.
    if boundary <= inherited_turns.saturating_add(FORWARD_BOUNDARY_TURN_THRESHOLD) {
        let mut offset = segment.seed_end_offset;
        loop {
            let remaining = limits.max_scan_bytes.saturating_sub(*scanned_bytes);
            let (record, bytes) = store
                .read_next_log_record_read_only_bounded(
                    session_id,
                    segment.segment_id,
                    offset,
                    remaining,
                )
                .map_err(map_history_store_error)?;
            *scanned_bytes = scanned_bytes
                .checked_add(bytes)
                .ok_or(RetainedHistoryReadError::ResourceLimit)?;
            let record = record.ok_or(RetainedHistoryReadError::CorruptLog)?;
            *scanned_entries = scanned_entries
                .checked_add(persisted_entry_units(std::slice::from_ref(&record.entry)))
                .ok_or(RetainedHistoryReadError::ResourceLimit)?;
            if *scanned_entries > limits.max_entries {
                return Err(RetainedHistoryReadError::ResourceLimit);
            }
            offset = record.end_offset;
            match record.entry {
                LogEntry::TurnEnd { turn_count, .. } if turn_count == boundary => {
                    return Ok(offset);
                }
                LogEntry::TurnEnd { turn_count, .. } if turn_count > boundary => {
                    return Err(RetainedHistoryReadError::CorruptLog);
                }
                _ => {}
            }
        }
    }

    let mut reader = store
        .open_retained_segment_reader(session_id, segment.segment_id, segment.file_len)
        .map_err(map_history_store_error)?;
    loop {
        let remaining = limits.max_scan_bytes.saturating_sub(*scanned_bytes);
        let (record, bytes) = reader
            .previous_record(remaining)
            .map_err(map_history_store_error)?;
        *scanned_bytes = scanned_bytes
            .checked_add(bytes)
            .ok_or(RetainedHistoryReadError::ResourceLimit)?;
        let record = record.ok_or(RetainedHistoryReadError::CorruptLog)?;
        *scanned_entries = scanned_entries
            .checked_add(persisted_entry_units(std::slice::from_ref(&record.entry)))
            .ok_or(RetainedHistoryReadError::ResourceLimit)?;
        if *scanned_entries > limits.max_entries {
            return Err(RetainedHistoryReadError::ResourceLimit);
        }
        match record.entry {
            LogEntry::TurnEnd { turn_count, .. } if turn_count == boundary => {
                return Ok(record.end_offset);
            }
            LogEntry::TurnEnd { turn_count, .. } if turn_count < boundary => {
                return Err(RetainedHistoryReadError::CorruptLog);
            }
            LogEntry::AnnotatedSegmentStart { .. } => {
                return Err(RetainedHistoryReadError::CorruptLog);
            }
            _ => {}
        }
    }
}

fn read_history_turns_backward(
    store: &WorkerSessionStore,
    session_id: SessionId,
    lineage: &[LoadedLineageSegment],
    start: Option<(HistoryTurnPosition, &str)>,
    turn_limit: usize,
    limits: RetainedHistoryReadLimits,
    scanned_bytes: &mut u64,
    scanned_entries: &mut usize,
) -> Result<Vec<PositionedHistoryTurn>, RetainedHistoryReadError> {
    let mut turns = Vec::new();
    let mut pending_entries = Vec::new();
    let mut seen_entries = HashSet::new();

    let (start_segment_index, start_position, cursor_turn_id) = match start {
        Some((position, turn_id)) => {
            let index = lineage
                .iter()
                .position(|segment| segment.segment_id == position.segment_id)
                .ok_or(RetainedHistoryReadError::InvalidCursor)?;
            (index, Some(position), Some(turn_id))
        }
        None => (0, None, None),
    };

    if let (Some(position), Some(turn_id)) = (start_position.as_ref(), cursor_turn_id) {
        let segment = &lineage[start_segment_index];
        if let Some(seed_index) = position.seed_entry_index {
            if start_segment_index + 1 != lineage.len()
                || position.before_offset != 0
                || position.record_end_offset.is_some()
                || segment
                    .seed_entries
                    .get(seed_index)
                    .is_none_or(|entry| entry.entry_id != turn_id || !is_user_entry(entry))
            {
                return Err(RetainedHistoryReadError::InvalidCursor);
            }
        } else {
            let record_end = position
                .record_end_offset
                .ok_or(RetainedHistoryReadError::InvalidCursor)?;
            let remaining = limits.max_scan_bytes.saturating_sub(*scanned_bytes);
            let record = store
                .read_log_record_range_read_only_bounded(
                    session_id,
                    segment.segment_id,
                    position.before_offset,
                    record_end,
                    remaining,
                )
                .map_err(map_history_cursor_store_error)?;
            *scanned_bytes = scanned_bytes
                .checked_add(record.end_offset - record.start_offset)
                .ok_or(RetainedHistoryReadError::ResourceLimit)?;
            if record.start_offset != position.before_offset
                || !project_history_record(session_id, segment.segment_id, &record.entry)
                    .iter()
                    .any(|entry| entry.entry_id == turn_id && is_user_entry(entry))
            {
                return Err(RetainedHistoryReadError::InvalidCursor);
            }
            *scanned_entries = scanned_entries
                .checked_add(persisted_entry_units(std::slice::from_ref(&record.entry)))
                .ok_or(RetainedHistoryReadError::ResourceLimit)?;
            if *scanned_entries > limits.max_entries {
                return Err(RetainedHistoryReadError::ResourceLimit);
            }
        }
    }

    for (segment_index, segment) in lineage.iter().enumerate().skip(start_segment_index) {
        let starting_here = segment_index == start_segment_index;
        if starting_here
            && start_position
                .as_ref()
                .and_then(|position| position.seed_entry_index)
                .is_some()
        {
            let seed_index = start_position
                .as_ref()
                .and_then(|position| position.seed_entry_index)
                .expect("checked seed cursor");
            if collect_history_entries_backward(
                &segment.seed_entries[..seed_index],
                segment.segment_id,
                0,
                None,
                true,
                turn_limit,
                &mut seen_entries,
                &mut pending_entries,
                &mut turns,
            ) {
                return Ok(turns);
            }
            continue;
        }

        let adopted_end = locate_adopted_segment_end(
            store,
            session_id,
            segment,
            limits,
            scanned_bytes,
            scanned_entries,
        )?;
        let before_offset = if starting_here {
            start_position
                .as_ref()
                .map(|position| position.before_offset)
                .unwrap_or(adopted_end)
        } else {
            adopted_end
        };
        if before_offset > adopted_end {
            return Err(RetainedHistoryReadError::InvalidCursor);
        }
        let mut reader = store
            .open_retained_segment_reader(session_id, segment.segment_id, before_offset)
            .map_err(map_history_store_error)?;
        loop {
            let remaining = limits.max_scan_bytes.saturating_sub(*scanned_bytes);
            let (record, bytes) = reader
                .previous_record(remaining)
                .map_err(map_history_store_error)?;
            *scanned_bytes = scanned_bytes
                .checked_add(bytes)
                .ok_or(RetainedHistoryReadError::ResourceLimit)?;
            let Some(record) = record else {
                break;
            };
            *scanned_entries = scanned_entries
                .checked_add(persisted_entry_units(std::slice::from_ref(&record.entry)))
                .ok_or(RetainedHistoryReadError::ResourceLimit)?;
            if *scanned_entries > limits.max_entries {
                return Err(RetainedHistoryReadError::ResourceLimit);
            }
            if matches!(record.entry, LogEntry::AnnotatedSegmentStart { .. }) {
                if record.start_offset != 0 {
                    return Err(RetainedHistoryReadError::CorruptLog);
                }
                continue;
            }
            let entries = project_history_record(session_id, segment.segment_id, &record.entry);
            if collect_history_entries_backward(
                &entries,
                segment.segment_id,
                record.start_offset,
                Some(record.end_offset),
                false,
                turn_limit,
                &mut seen_entries,
                &mut pending_entries,
                &mut turns,
            ) {
                return Ok(turns);
            }
        }

        if segment_index + 1 == lineage.len()
            && collect_history_entries_backward(
                &segment.seed_entries,
                segment.segment_id,
                0,
                None,
                true,
                turn_limit,
                &mut seen_entries,
                &mut pending_entries,
                &mut turns,
            )
        {
            return Ok(turns);
        }
    }
    Ok(turns)
}

#[allow(clippy::too_many_arguments)]
fn collect_history_entries_backward(
    entries: &[SessionSnapshotEntry],
    segment_id: SegmentId,
    before_offset: u64,
    record_end_offset: Option<u64>,
    seed: bool,
    turn_limit: usize,
    seen_entries: &mut HashSet<String>,
    pending_entries: &mut Vec<SessionSnapshotEntry>,
    turns: &mut Vec<PositionedHistoryTurn>,
) -> bool {
    for (index, entry) in entries.iter().enumerate().rev() {
        if entry.provenance == SessionEntryProvenance::DerivedSummary
            || !seen_entries.insert(entry.entry_id.clone())
        {
            continue;
        }
        pending_entries.push(entry.clone());
        if is_user_entry(entry) {
            let mut turn_entries = std::mem::take(pending_entries);
            turn_entries.reverse();
            turns.push(PositionedHistoryTurn {
                turn: SessionConversationTurn {
                    turn_id: entry.entry_id.clone(),
                    entries: turn_entries,
                },
                position: HistoryTurnPosition {
                    segment_id,
                    before_offset,
                    record_end_offset,
                    seed_entry_index: seed.then_some(index),
                },
            });
            if turns.len() >= turn_limit {
                return true;
            }
        }
    }
    false
}

fn project_history_record(
    session_id: SessionId,
    segment_id: SegmentId,
    record: &LogEntry,
) -> Vec<SessionSnapshotEntry> {
    project_session_snapshot_for_segment(session_id, Some(segment_id), std::slice::from_ref(record))
        .entries
        .into_iter()
        .filter(|entry| entry.provenance != SessionEntryProvenance::DerivedSummary)
        .collect()
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

fn map_history_cursor_store_error(error: StoreError) -> RetainedHistoryReadError {
    match error {
        StoreError::ReadLimitExceeded => RetainedHistoryReadError::ResourceLimit,
        _ => RetainedHistoryReadError::InvalidCursor,
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

fn map_history_attachment_error(error: RetainedHistoryReadError) -> RetainedAttachmentReadError {
    match error {
        RetainedHistoryReadError::RetentionMissing => RetainedAttachmentReadError::RetentionMissing,
        RetainedHistoryReadError::ActivePointerMissing => {
            RetainedAttachmentReadError::ActivePointerMissing
        }
        RetainedHistoryReadError::MigrationRequired => {
            RetainedAttachmentReadError::MigrationRequired
        }
        RetainedHistoryReadError::CorruptLog | RetainedHistoryReadError::InvalidCursor => {
            RetainedAttachmentReadError::CorruptLog
        }
        RetainedHistoryReadError::StorageUnavailable => {
            RetainedAttachmentReadError::StorageUnavailable
        }
        RetainedHistoryReadError::ResourceLimit => RetainedAttachmentReadError::ResourceLimit,
    }
}

fn map_attachment_worker_store_error(error: WorkerStoreError) -> RetainedAttachmentReadError {
    match error {
        WorkerStoreError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
            RetainedAttachmentReadError::RetentionMissing
        }
        WorkerStoreError::Serde(_) | WorkerStoreError::InvalidWorkerName(_) => {
            RetainedAttachmentReadError::CorruptLog
        }
        WorkerStoreError::Io(_) => RetainedAttachmentReadError::StorageUnavailable,
    }
}

fn map_attachment_store_error(error: StoreError) -> RetainedAttachmentReadError {
    match error {
        StoreError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
            RetainedAttachmentReadError::RetentionMissing
        }
        StoreError::Corrupt { message, .. } if message.contains("requires migration") => {
            RetainedAttachmentReadError::MigrationRequired
        }
        StoreError::ReadLimitExceeded => RetainedAttachmentReadError::ResourceLimit,
        StoreError::Io(_) => RetainedAttachmentReadError::StorageUnavailable,
        StoreError::Serde(_) | StoreError::Corrupt { .. } | StoreError::NotFound(_) => {
            RetainedAttachmentReadError::CorruptLog
        }
        _ => RetainedAttachmentReadError::CorruptLog,
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

pub(crate) fn project_session_snapshot_for_segment(
    session_id: SessionId,
    segment_id: Option<SegmentId>,
    log: &[LogEntry],
) -> SessionSnapshot {
    let mut session_key = session_id;
    let mut entries = Vec::new();
    let mut total_turn_count = 0usize;
    let mut active_run_turn_count = None;

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
                total_turn_count = 0;
                active_run_turn_count = None;
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
                if let Some(data) = project_item(&entry.metadata.entry_id.0, &entry.item) {
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
            LogEntry::RunYielded {
                ts,
                entry_id,
                reason,
                active_run_turn_count: count,
            } => {
                active_run_turn_count = Some(*count);
                entries.push(run_transition_entry(
                    entry_id.as_ref(),
                    &session_key,
                    segment_id.as_ref(),
                    log_index,
                    *ts,
                    SessionSnapshotEntryData::RunYielded {
                        reason: *reason,
                        active_run_turn_count: *count,
                    },
                ));
            }
            LogEntry::RunResumed {
                ts,
                entry_id,
                source,
                active_run_turn_count: count,
            } => {
                active_run_turn_count = Some(*count);
                entries.push(run_transition_entry(
                    entry_id.as_ref(),
                    &session_key,
                    segment_id.as_ref(),
                    log_index,
                    *ts,
                    SessionSnapshotEntryData::RunResumed {
                        source: *source,
                        active_run_turn_count: *count,
                    },
                ));
            }
            LogEntry::RunCancelled { ts, entry_id } => {
                active_run_turn_count = None;
                entries.push(run_transition_entry(
                    entry_id.as_ref(),
                    &session_key,
                    segment_id.as_ref(),
                    log_index,
                    *ts,
                    SessionSnapshotEntryData::RunCancelled,
                ));
            }
            LogEntry::RunErrored {
                ts,
                entry_id,
                message,
                failure,
                ..
            } => {
                active_run_turn_count = None;
                entries.push(run_transition_entry(
                    entry_id.as_ref(),
                    &session_key,
                    segment_id.as_ref(),
                    log_index,
                    *ts,
                    SessionSnapshotEntryData::RunError {
                        message: message.clone(),
                        failure: *failure,
                    },
                ));
            }
            LogEntry::Invoke { .. } => active_run_turn_count = Some(0),
            LogEntry::TurnEnd { turn_count, .. } => {
                if let Some(active) = &mut active_run_turn_count {
                    *active += turn_count.saturating_sub(total_turn_count);
                }
                total_turn_count = *turn_count;
            }
            LogEntry::RunCompleted {
                ts,
                interrupted,
                result,
                active_run_turn_count: persisted_count,
            } => {
                if *interrupted && matches!(result, agen::EngineResult::Yielded) {
                    let count = (*persisted_count)
                        .or(active_run_turn_count)
                        .unwrap_or_default();
                    active_run_turn_count = Some(count);
                    entries.push(legacy_entry(
                        &session_key,
                        segment_id.as_ref(),
                        log_index,
                        0,
                        *ts,
                        SessionSnapshotEntryData::RunYielded {
                            reason: protocol::RunYieldReason::Compaction,
                            active_run_turn_count: count,
                        },
                    ));
                } else if *interrupted && matches!(result, agen::EngineResult::Paused) {
                    active_run_turn_count = (*persisted_count).or(active_run_turn_count);
                } else {
                    active_run_turn_count = None;
                }
            }
            LogEntry::ActiveRunCheckpoint {
                active_turn_count,
                total_turn_count: persisted_total,
                ..
            } => {
                active_run_turn_count = Some(*active_turn_count);
                total_turn_count = *persisted_total;
            }
            LogEntry::PausedTurnAbandoned { .. } => active_run_turn_count = None,
            // Configuration, usage, and extension state are controller/storage
            // authority rather than committed conversation.
            LogEntry::ConfigChanged { .. }
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
            let Some(data) = project_item(&entry.metadata.entry_id.0, &entry.item) else {
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

fn run_transition_entry(
    persisted_entry_id: Option<&crate::LoggedSessionHistoryEntryId>,
    session_key: &SessionId,
    segment_id: Option<&SegmentId>,
    log_index: usize,
    timestamp: u64,
    data: SessionSnapshotEntryData,
) -> SessionSnapshotEntry {
    SessionSnapshotEntry {
        entry_id: persisted_entry_id
            .map(|entry_id| entry_id.0.clone())
            .unwrap_or_else(|| legacy_entry_id(session_key, segment_id, log_index, 0)),
        timestamp,
        provenance: SessionEntryProvenance::LegacyUnknown,
        derived_from: Vec::new(),
        data,
    }
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

pub fn session_tool_attachment_id(
    entry_id: &str,
    attachment_index: usize,
    media_type: &str,
    data: &[u8],
) -> String {
    let mut hash = Sha256::new();
    hash.update(b"yoi-session-tool-attachment-v1\0");
    hash.update((entry_id.len() as u64).to_be_bytes());
    hash.update(entry_id.as_bytes());
    hash.update((attachment_index as u64).to_be_bytes());
    hash.update((media_type.len() as u64).to_be_bytes());
    hash.update(media_type.as_bytes());
    hash.update((data.len() as u64).to_be_bytes());
    hash.update(data);
    URL_SAFE_NO_PAD.encode(hash.finalize())
}

fn project_item(entry_id: &str, item: &LoggedItem) -> Option<SessionSnapshotEntryData> {
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
                .enumerate()
                .map(|(attachment_index, attachment)| match attachment {
                    crate::logged_item::LoggedAttachment::Image { mime_type, data } => {
                        SessionToolAttachment {
                            attachment_id: session_tool_attachment_id(
                                entry_id,
                                attachment_index,
                                mime_type,
                                data,
                            ),
                            media_type: mime_type.clone(),
                            byte_len: data.len() as u64,
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
        assert_eq!(newest.compact_ancestor_lineage_ids.len(), 2);
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
        let store = WorkerSessionStore::open_read_only(aggregate_root.join("session")).unwrap();
        let (_, source_lineage_id, _, _) = load_adopted_lineage(
            &store,
            session_id,
            source_segment,
            RetainedHistoryReadLimits::default(),
        )
        .unwrap();
        let parent = page.parent_lineage.as_ref().unwrap();
        assert_eq!(parent.lineage_id, source_lineage_id);
        let adopted_turn = parent.adopted_through_turn.as_ref().unwrap();
        assert_eq!(adopted_turn.turn_id, "user-1");
        assert_eq!(
            adopted_turn
                .entries
                .iter()
                .map(|entry| entry.entry_id.as_str())
                .collect::<Vec<_>>(),
            vec!["user-1", "assistant-1-boundary"]
        );
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
        let mut tampered = decode_history_cursor(&cursor).unwrap();
        tampered.before_turn_id = "user-1".to_string();
        let tampered = encode_history_cursor(&tampered).unwrap();
        assert_eq!(
            read_retained_session_history_page(
                &aggregate_a,
                "worker-a",
                Some(&tampered),
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
        let mut appended = Vec::new();
        append_turn(&mut appended, 7);
        let session = WorkerSessionStore::new(aggregate_a.join("session")).unwrap();
        for entry in appended {
            session
                .append(active.session_id, old_segment, &entry)
                .unwrap();
        }
        let older = read_retained_session_history_page(
            &aggregate_a,
            "worker-a",
            Some(&cursor),
            None,
            RetainedHistoryReadLimits::default(),
        )
        .unwrap();
        assert_eq!(turn_ids(&older), vec!["user-1"]);

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
    fn retained_history_reads_only_a_bounded_suffix_for_each_page() {
        let worker_name = "worker-bounded-backward-pages";
        let session_id = crate::new_session_id();
        let segment_id = crate::new_segment_id();
        let mut log = vec![segment_start(session_id, Vec::new(), None, None)];
        for turn in 1..=200 {
            append_turn(&mut log, turn);
        }
        let (_root, aggregate_root) =
            persist_history_fixture(worker_name, session_id, segment_id, vec![(segment_id, log)]);
        let limits = RetainedHistoryReadLimits {
            max_scan_bytes: 64 * 1024,
            ..RetainedHistoryReadLimits::default()
        };
        assert!(
            std::fs::metadata(
                aggregate_root
                    .join("session/segments")
                    .join(format!("{segment_id}.jsonl")),
            )
            .unwrap()
            .len()
                > limits.max_scan_bytes,
            "the fixture must be larger than one page's complete scan budget",
        );

        let newest =
            read_retained_session_history_page(&aggregate_root, worker_name, None, None, limits)
                .unwrap();
        assert_eq!(
            turn_ids(&newest),
            vec!["user-196", "user-197", "user-198", "user-199", "user-200"]
        );

        let earlier = read_retained_session_history_page(
            &aggregate_root,
            worker_name,
            newest.next_cursor.as_deref(),
            None,
            limits,
        )
        .unwrap();
        assert_eq!(
            turn_ids(&earlier),
            vec!["user-191", "user-192", "user-193", "user-194", "user-195"]
        );
    }

    #[test]
    fn retained_history_does_not_scan_a_fork_sources_unadopted_suffix() {
        let worker_name = "worker-bounded-fork-prefix";
        let session_id = crate::new_session_id();
        let source_segment = crate::new_segment_id();
        let active_segment = crate::new_segment_id();
        let mut source_log = vec![segment_start(session_id, Vec::new(), None, None)];
        for turn in 1..=200 {
            append_turn(&mut source_log, turn);
        }
        let mut active_log = vec![segment_start(
            session_id,
            Vec::new(),
            Some(SegmentOrigin {
                segment_id: source_segment,
                at_turn_index: 5,
            }),
            None,
        )];
        append_turn(&mut active_log, 201);
        let (_root, aggregate_root) = persist_history_fixture(
            worker_name,
            session_id,
            active_segment,
            vec![(source_segment, source_log), (active_segment, active_log)],
        );
        let limits = RetainedHistoryReadLimits {
            max_scan_bytes: 64 * 1024,
            ..RetainedHistoryReadLimits::default()
        };
        assert!(
            std::fs::metadata(
                aggregate_root
                    .join("session/segments")
                    .join(format!("{source_segment}.jsonl")),
            )
            .unwrap()
            .len()
                > limits.max_scan_bytes,
        );

        let page =
            read_retained_session_history_page(&aggregate_root, worker_name, None, None, limits)
                .unwrap();
        assert_eq!(
            turn_ids(&page),
            vec!["user-2", "user-3", "user-4", "user-5", "user-201"]
        );
        assert!(page.has_more);
    }

    #[test]
    fn retained_history_pages_within_one_root_seed_record() {
        let worker_name = "worker-root-seed-pages";
        let session_id = crate::new_session_id();
        let segment_id = crate::new_segment_id();
        let history = (1..=12)
            .flat_map(|turn| [user_message(turn), assistant_message(turn, "final")])
            .collect();
        let log = vec![
            segment_start(session_id, history, None, None),
            LogEntry::InputSegmentsCheckpoint {
                ts: 2,
                user_segments: (1..=12)
                    .map(|turn| {
                        vec![Segment::Text {
                            content: format!("typed request {turn}"),
                        }]
                    })
                    .collect(),
            },
        ];
        let (_root, aggregate_root) =
            persist_history_fixture(worker_name, session_id, segment_id, vec![(segment_id, log)]);

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
        assert_eq!(
            newest.turns[0].entries[0].data,
            SessionSnapshotEntryData::UserInput {
                segments: vec![Segment::Text {
                    content: "typed request 8".to_string(),
                }],
            }
        );
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
    }

    #[test]
    fn retained_history_keeps_each_nested_lineage_edge_boundary_local() {
        let worker_name = "worker-nested-boundaries";
        let session_id = crate::new_session_id();
        let root_segment = crate::new_segment_id();
        let middle_segment = crate::new_segment_id();
        let active_segment = crate::new_segment_id();
        let mut root_log = vec![segment_start(session_id, Vec::new(), None, None)];
        for turn in 1..=3 {
            append_turn(&mut root_log, turn);
        }
        let middle_log = vec![segment_start(
            session_id,
            vec![user_message(3), assistant_message(3, "final")],
            Some(SegmentOrigin {
                segment_id: root_segment,
                at_turn_index: 3,
            }),
            None,
        )];
        let mut active_log = vec![segment_start(
            session_id,
            Vec::new(),
            Some(SegmentOrigin {
                segment_id: middle_segment,
                at_turn_index: 0,
            }),
            None,
        )];
        append_turn(&mut active_log, 4);
        let (_root, aggregate_root) = persist_history_fixture(
            worker_name,
            session_id,
            active_segment,
            vec![
                (root_segment, root_log),
                (middle_segment, middle_log),
                (active_segment, active_log),
            ],
        );

        let page = read_retained_session_history_page(
            &aggregate_root,
            worker_name,
            None,
            None,
            RetainedHistoryReadLimits::default(),
        )
        .unwrap();
        assert_eq!(
            turn_ids(&page),
            vec!["user-1", "user-2", "user-3", "user-4"]
        );
    }

    #[test]
    fn retained_history_resolves_an_immediate_consecutive_compact_at_the_seed() {
        let worker_name = "worker-consecutive-compact";
        let session_id = crate::new_session_id();
        let root_segment = crate::new_segment_id();
        let middle_segment = crate::new_segment_id();
        let active_segment = crate::new_segment_id();
        let mut root_log = vec![segment_start(session_id, Vec::new(), None, None)];
        for turn in 1..=5 {
            append_turn(&mut root_log, turn);
        }
        let middle_log = vec![segment_start(
            session_id,
            Vec::new(),
            None,
            Some(SegmentOrigin {
                segment_id: root_segment,
                at_turn_index: 5,
            }),
        )];
        let active_log = vec![segment_start(
            session_id,
            Vec::new(),
            None,
            Some(SegmentOrigin {
                segment_id: middle_segment,
                at_turn_index: 5,
            }),
        )];
        let (_root, aggregate_root) = persist_history_fixture(
            worker_name,
            session_id,
            active_segment,
            vec![
                (root_segment, root_log),
                (middle_segment, middle_log),
                (active_segment, active_log),
            ],
        );

        let page = read_retained_session_history_page(
            &aggregate_root,
            worker_name,
            None,
            None,
            RetainedHistoryReadLimits::default(),
        )
        .unwrap();
        assert_eq!(
            turn_ids(&page),
            vec!["user-1", "user-2", "user-3", "user-4", "user-5"]
        );
        assert_eq!(page.compact_ancestor_lineage_ids.len(), 2);
    }

    #[test]
    fn retained_history_finds_a_late_adopted_boundary_from_the_tail() {
        let worker_name = "worker-late-adopted-boundary";
        let session_id = crate::new_session_id();
        let source_segment = crate::new_segment_id();
        let active_segment = crate::new_segment_id();
        let mut source_log = vec![segment_start(session_id, Vec::new(), None, None)];
        append_turn(&mut source_log, 1);
        source_log.insert(
            3,
            LogEntry::AnnotatedAssistantItem {
                ts: 12,
                entry: history_message(
                    "assistant-1-large",
                    LoggedRole::Assistant,
                    "x".repeat(300 * 1024),
                    LoggedSessionHistoryOrigin::LegacyUnknown,
                ),
            },
        );
        for turn in 2..=101 {
            append_turn(&mut source_log, turn);
        }
        let active_log = vec![segment_start(
            session_id,
            Vec::new(),
            Some(SegmentOrigin {
                segment_id: source_segment,
                at_turn_index: 100,
            }),
            None,
        )];
        let (_root, aggregate_root) = persist_history_fixture(
            worker_name,
            session_id,
            active_segment,
            vec![(source_segment, source_log), (active_segment, active_log)],
        );
        let limits = RetainedHistoryReadLimits {
            max_scan_bytes: 256 * 1024,
            ..RetainedHistoryReadLimits::default()
        };
        assert!(
            std::fs::metadata(
                aggregate_root
                    .join("session/segments")
                    .join(format!("{source_segment}.jsonl")),
            )
            .unwrap()
            .len()
                > limits.max_scan_bytes,
        );

        let page =
            read_retained_session_history_page(&aggregate_root, worker_name, None, None, limits)
                .unwrap();
        assert_eq!(
            turn_ids(&page),
            vec!["user-96", "user-97", "user-98", "user-99", "user-100"]
        );
        assert!(page.has_more);
    }

    #[test]
    fn retained_history_shares_one_entry_budget_with_parent_boundary_lookup() {
        let worker_name = "worker-shared-parent-budget";
        let session_id = crate::new_session_id();
        let source_segment = crate::new_segment_id();
        let active_segment = crate::new_segment_id();
        let mut source_log = vec![segment_start(session_id, Vec::new(), None, None)];
        for turn in 1..=5 {
            append_turn(&mut source_log, turn);
        }
        let mut active_log = vec![segment_start(
            session_id,
            Vec::new(),
            Some(SegmentOrigin {
                segment_id: source_segment,
                at_turn_index: 5,
            }),
            None,
        )];
        for turn in 6..=10 {
            append_turn(&mut active_log, turn);
        }
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
            Some(1),
            RetainedHistoryReadLimits {
                max_entries: 40,
                ..RetainedHistoryReadLimits::default()
            },
        )
        .unwrap_err();
        assert_eq!(error, RetainedHistoryReadError::ResourceLimit);

        let page = read_retained_session_history_page(
            &aggregate_root,
            worker_name,
            None,
            Some(1),
            RetainedHistoryReadLimits {
                max_entries: 100,
                ..RetainedHistoryReadLimits::default()
            },
        )
        .unwrap();
        assert_eq!(turn_ids(&page), vec!["user-10"]);
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
        let body = "x".repeat(128 * 1024);
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
    fn legacy_run_completed_yield_projects_as_nonterminal_run_yielded() {
        let session_id = crate::new_session_id();
        let legacy: LogEntry = serde_json::from_value(serde_json::json!({
            "kind": "run_completed",
            "ts": 30,
            "interrupted": true,
            "result": "yielded"
        }))
        .unwrap();
        let snapshot = project_session_snapshot(
            session_id,
            &[
                LogEntry::Invoke {
                    ts: 10,
                    trigger: protocol::InvokeKind::UserSend,
                },
                LogEntry::TurnEnd {
                    ts: 20,
                    turn_count: 4,
                },
                legacy,
            ],
        );

        assert_eq!(snapshot.entries.len(), 1);
        assert!(matches!(
            snapshot.entries[0].data,
            SessionSnapshotEntryData::RunYielded {
                reason: protocol::RunYieldReason::Compaction,
                active_run_turn_count: 4,
            }
        ));
    }

    fn run_entry_ids(entries: &[SessionSnapshotEntry]) -> Vec<String> {
        entries
            .iter()
            .filter(|entry| {
                matches!(
                    entry.data,
                    SessionSnapshotEntryData::RunYielded { .. }
                        | SessionSnapshotEntryData::RunResumed { .. }
                        | SessionSnapshotEntryData::RunCancelled
                        | SessionSnapshotEntryData::RunError { .. }
                )
            })
            .map(|entry| entry.entry_id.clone())
            .collect()
    }

    #[test]
    fn saved_run_transition_ids_match_snapshot_retained_and_paged_history() {
        let worker_name = "worker-run-identities";
        let session_id = crate::new_session_id();
        let segment_id = crate::new_segment_id();
        let initial = vec![
            segment_start(session_id, Vec::new(), None, None),
            LogEntry::AnnotatedUserInput {
                ts: 2,
                segments: vec![Segment::text("compact")],
                history: vec![user_message(1)],
                extensions: Vec::new(),
            },
        ];
        let (_root, aggregate_root) = persist_history_fixture(
            worker_name,
            session_id,
            segment_id,
            vec![(segment_id, initial)],
        );
        let store = WorkerSessionStore::new(aggregate_root.join("session")).unwrap();
        let transitions = [
            LogEntry::RunYielded {
                ts: 10,
                entry_id: None,
                reason: protocol::RunYieldReason::Compaction,
                active_run_turn_count: 2,
            },
            LogEntry::RunResumed {
                ts: 10,
                entry_id: None,
                source: protocol::RunResumeSource::Compaction,
                active_run_turn_count: 2,
            },
            LogEntry::RunErrored {
                ts: 10,
                entry_id: None,
                interrupted: false,
                message: "mid-run compaction failed: unavailable".into(),
                failure: Some(protocol::RunFailureKind::Compaction),
            },
            LogEntry::RunErrored {
                ts: 10,
                entry_id: None,
                interrupted: false,
                message: "mid-run compaction failed: unavailable".into(),
                failure: Some(protocol::RunFailureKind::Compaction),
            },
            LogEntry::RunCancelled {
                ts: 10,
                entry_id: None,
            },
        ];
        for entry in transitions {
            crate::segment::append_entry(&store, session_id, segment_id, entry).unwrap();
        }

        let stored = store.read_all(session_id, segment_id).unwrap();
        let stored_ids = run_entry_ids(
            &project_session_snapshot_for_segment(session_id, Some(segment_id), &stored).entries,
        );
        assert_eq!(stored_ids.len(), 5);
        assert_eq!(stored_ids.iter().collect::<HashSet<_>>().len(), 5);
        assert!(
            stored_ids
                .iter()
                .all(|entry_id| !entry_id.starts_with("l-run-"))
        );

        let retained = read_retained_session_snapshot(
            &aggregate_root,
            worker_name,
            DEFAULT_RETAINED_SNAPSHOT_MAX_BYTES,
        )
        .unwrap();
        assert_eq!(run_entry_ids(&retained.snapshot.entries), stored_ids);

        let page = read_retained_session_history_page(
            &aggregate_root,
            worker_name,
            None,
            Some(1),
            RetainedHistoryReadLimits::default(),
        )
        .unwrap();
        assert_eq!(page.turns.len(), 1);
        assert_eq!(run_entry_ids(&page.turns[0].entries), stored_ids);

        let reconnect = read_retained_session_snapshot(
            &aggregate_root,
            worker_name,
            DEFAULT_RETAINED_SNAPSHOT_MAX_BYTES,
        )
        .unwrap();
        assert_eq!(run_entry_ids(&reconnect.snapshot.entries), stored_ids);
    }

    #[test]
    fn run_transition_ids_survive_segment_lineage_and_history_page_boundaries() {
        let worker_name = "worker-run-lineage-identities";
        let session_id = crate::new_session_id();
        let source_segment = crate::new_segment_id();
        let active_segment = crate::new_segment_id();
        let source = vec![
            segment_start(session_id, Vec::new(), None, None),
            LogEntry::AnnotatedUserInput {
                ts: 2,
                segments: vec![Segment::text("source turn")],
                history: vec![user_message(1)],
                extensions: Vec::new(),
            },
        ];
        let (_root, aggregate_root) = persist_history_fixture(
            worker_name,
            session_id,
            source_segment,
            vec![(source_segment, source)],
        );
        let store = WorkerSessionStore::new(aggregate_root.join("session")).unwrap();
        crate::segment::append_entry(
            &store,
            session_id,
            source_segment,
            LogEntry::RunYielded {
                ts: 3,
                entry_id: None,
                reason: protocol::RunYieldReason::Compaction,
                active_run_turn_count: 1,
            },
        )
        .unwrap();
        store
            .append(
                session_id,
                source_segment,
                &LogEntry::TurnEnd {
                    ts: 4,
                    turn_count: 1,
                },
            )
            .unwrap();
        store
            .create_segment(
                session_id,
                active_segment,
                &[
                    segment_start(
                        session_id,
                        vec![user_message(1)],
                        None,
                        Some(SegmentOrigin {
                            segment_id: source_segment,
                            at_turn_index: 1,
                        }),
                    ),
                    LogEntry::AnnotatedUserInput {
                        ts: 5,
                        segments: vec![Segment::text("active turn")],
                        history: vec![user_message(2)],
                        extensions: Vec::new(),
                    },
                ],
            )
            .unwrap();
        for entry in [
            LogEntry::RunResumed {
                ts: 6,
                entry_id: None,
                source: protocol::RunResumeSource::Compaction,
                active_run_turn_count: 1,
            },
            LogEntry::RunErrored {
                ts: 7,
                entry_id: None,
                interrupted: false,
                message: "mid-run compaction failed".into(),
                failure: Some(protocol::RunFailureKind::Compaction),
            },
        ] {
            crate::segment::append_entry(&store, session_id, active_segment, entry).unwrap();
        }
        WorkerAggregateStore::new(&aggregate_root, worker_name)
            .unwrap()
            .write(&crate::WorkerMetadata::new(
                worker_name,
                Some(crate::WorkerActiveSegmentRef::active_segment(
                    session_id,
                    active_segment,
                )),
            ))
            .unwrap();

        let active_ids = run_entry_ids(
            &project_session_snapshot_for_segment(
                session_id,
                Some(active_segment),
                &store.read_all(session_id, active_segment).unwrap(),
            )
            .entries,
        );
        let source_ids = run_entry_ids(
            &project_session_snapshot_for_segment(
                session_id,
                Some(source_segment),
                &store.read_all(session_id, source_segment).unwrap(),
            )
            .entries,
        );
        assert_eq!(active_ids.len(), 2);
        assert_eq!(source_ids.len(), 1);

        let newest = read_retained_session_history_page(
            &aggregate_root,
            worker_name,
            None,
            Some(1),
            RetainedHistoryReadLimits::default(),
        )
        .unwrap();
        assert_eq!(run_entry_ids(&newest.turns[0].entries), active_ids);
        assert!(newest.has_more);
        let older = read_retained_session_history_page(
            &aggregate_root,
            worker_name,
            newest.next_cursor.as_deref(),
            Some(1),
            RetainedHistoryReadLimits::default(),
        )
        .unwrap();
        assert_eq!(run_entry_ids(&older.turns[0].entries), source_ids);
    }

    #[test]
    fn legacy_run_transition_ids_use_stable_distinct_record_positions() {
        let worker_name = "worker-legacy-run-identities";
        let session_id = crate::new_session_id();
        let segment_id = crate::new_segment_id();
        let log = vec![
            segment_start(session_id, Vec::new(), None, None),
            LogEntry::AnnotatedUserInput {
                ts: 2,
                segments: vec![Segment::text("legacy")],
                history: vec![user_message(1)],
                extensions: Vec::new(),
            },
            LogEntry::RunErrored {
                ts: 10,
                entry_id: None,
                interrupted: false,
                message: "same legacy failure".into(),
                failure: None,
            },
            LogEntry::RunErrored {
                ts: 10,
                entry_id: None,
                interrupted: false,
                message: "same legacy failure".into(),
                failure: None,
            },
        ];
        let (_root, aggregate_root) =
            persist_history_fixture(worker_name, session_id, segment_id, vec![(segment_id, log)]);
        let log_path = aggregate_root
            .join("session")
            .join("segments")
            .join(format!("{segment_id}.jsonl"));
        let persisted_before = std::fs::read(&log_path).unwrap();
        assert!(!String::from_utf8_lossy(&persisted_before).contains("entry_id\":\"l-run-"));

        let retained = read_retained_session_snapshot(
            &aggregate_root,
            worker_name,
            DEFAULT_RETAINED_SNAPSHOT_MAX_BYTES,
        )
        .unwrap();
        let snapshot_ids = run_entry_ids(&retained.snapshot.entries);
        assert_eq!(snapshot_ids.len(), 2);
        assert_ne!(snapshot_ids[0], snapshot_ids[1]);
        assert!(
            snapshot_ids
                .iter()
                .all(|entry_id| entry_id.starts_with("l-run-"))
        );

        let page = read_retained_session_history_page(
            &aggregate_root,
            worker_name,
            None,
            Some(1),
            RetainedHistoryReadLimits::default(),
        )
        .unwrap();
        assert_eq!(run_entry_ids(&page.turns[0].entries), snapshot_ids);
        let reread = read_retained_session_snapshot(
            &aggregate_root,
            worker_name,
            DEFAULT_RETAINED_SNAPSHOT_MAX_BYTES,
        )
        .unwrap();
        assert_eq!(run_entry_ids(&reread.snapshot.entries), snapshot_ids);
        assert_eq!(std::fs::read(log_path).unwrap(), persisted_before);
    }

    #[test]
    fn run_transitions_project_with_timestamps_and_typed_causes() {
        let session_id = crate::new_session_id();
        let log = vec![
            LogEntry::RunYielded {
                ts: 10,
                entry_id: None,
                reason: protocol::RunYieldReason::Compaction,
                active_run_turn_count: 2,
            },
            LogEntry::RunResumed {
                ts: 20,
                entry_id: None,
                source: protocol::RunResumeSource::Compaction,
                active_run_turn_count: 2,
            },
            LogEntry::RunErrored {
                ts: 30,
                entry_id: None,
                interrupted: false,
                message: "mid-run compaction failed: unavailable".into(),
                failure: Some(protocol::RunFailureKind::Compaction),
            },
        ];

        let snapshot = project_session_snapshot(session_id, &log);
        assert_eq!(snapshot.entries.len(), 3);
        assert_eq!(snapshot.entries[0].timestamp, 10);
        assert!(matches!(
            snapshot.entries[0].data,
            SessionSnapshotEntryData::RunYielded {
                reason: protocol::RunYieldReason::Compaction,
                active_run_turn_count: 2,
            }
        ));
        assert!(matches!(
            snapshot.entries[1].data,
            SessionSnapshotEntryData::RunResumed {
                source: protocol::RunResumeSource::Compaction,
                active_run_turn_count: 2,
            }
        ));
        assert!(matches!(
            snapshot.entries[2].data,
            SessionSnapshotEntryData::RunError {
                failure: Some(protocol::RunFailureKind::Compaction),
                ..
            }
        ));
    }

    #[test]
    fn cancelled_run_projects_as_a_non_failure_terminal() {
        let snapshot = project_session_snapshot(
            crate::new_session_id(),
            &[
                LogEntry::Invoke {
                    ts: 10,
                    trigger: protocol::InvokeKind::UserSend,
                },
                LogEntry::RunCancelled {
                    ts: 20,
                    entry_id: None,
                },
            ],
        );

        assert_eq!(snapshot.entries.len(), 1);
        assert_eq!(snapshot.entries[0].timestamp, 20);
        assert!(matches!(
            snapshot.entries[0].data,
            SessionSnapshotEntryData::RunCancelled
        ));
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
    fn retained_attachment_resolves_from_an_adopted_compaction_ancestor() {
        let worker_name = "worker-ancestor-image";
        let session_id = crate::new_session_id();
        let source_segment = crate::new_segment_id();
        let active_segment = crate::new_segment_id();
        let image_entry = LoggedHistoryEntry {
            item: LoggedItem::ToolResult {
                call_id: "call-ancestor-image".into(),
                summary: "ancestor screenshot".into(),
                content: None,
                attachments: vec![crate::logged_item::LoggedAttachment::Image {
                    mime_type: "image/png".into(),
                    data: vec![7, 8, 9],
                }],
                disposition: agen::tool::ToolResultDisposition::Success,
                is_error: false,
            },
            metadata: LoggedSessionHistoryMetadata {
                entry_id: LoggedSessionHistoryEntryId("ancestor-image".into()),
                origin: LoggedSessionHistoryOrigin::LegacyUnknown,
                derivation: None,
            },
        };
        let mut source_log = vec![segment_start(session_id, Vec::new(), None, None)];
        source_log.push(LogEntry::AnnotatedUserInput {
            ts: 2,
            segments: vec![Segment::Text {
                content: "show image".into(),
            }],
            history: vec![user_message(1)],
            extensions: Vec::new(),
        });
        source_log.push(LogEntry::AnnotatedToolResult {
            ts: 3,
            entry: image_entry,
        });
        source_log.push(LogEntry::TurnEnd {
            ts: 4,
            turn_count: 1,
        });
        let active_log = vec![segment_start(
            session_id,
            Vec::new(),
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
        let attachment = page.turns[0]
            .entries
            .iter()
            .find_map(|entry| match &entry.data {
                SessionSnapshotEntryData::ToolResult { attachments, .. } => attachments.first(),
                _ => None,
            })
            .expect("ancestor attachment reference");
        let body = read_retained_session_attachment(
            &aggregate_root,
            worker_name,
            &session_id.to_string(),
            &attachment.attachment_id,
            DEFAULT_RETAINED_HISTORY_MAX_SCAN_BYTES,
            10 * 1024 * 1024,
        )
        .unwrap();
        assert_eq!(body.media_type, "image/png");
        assert_eq!(body.data, vec![7, 8, 9]);
    }

    #[test]
    fn large_legacy_inline_images_project_as_bounded_refs_and_fetch_from_retained_storage() {
        let worker_name = "worker-images";
        let session_id = crate::new_session_id();
        let segment_id = crate::new_segment_id();
        // Approximate the decoded byte sizes of the four W-296 screenshots.
        let sizes = [3_475_194_usize, 3_459_051, 5_735_340, 5_703_924];
        let attachments = sizes
            .iter()
            .enumerate()
            .map(
                |(index, size)| crate::logged_item::LoggedAttachment::Image {
                    mime_type: "image/png".into(),
                    data: vec![index as u8 + 1; *size],
                },
            )
            .collect();
        let tool_result = LoggedHistoryEntry {
            item: LoggedItem::ToolResult {
                call_id: "call-images".into(),
                summary: "four screenshots".into(),
                content: None,
                attachments,
                disposition: agen::tool::ToolResultDisposition::Success,
                is_error: false,
            },
            metadata: LoggedSessionHistoryMetadata {
                entry_id: LoggedSessionHistoryEntryId("tool-images".into()),
                origin: LoggedSessionHistoryOrigin::LegacyUnknown,
                derivation: None,
            },
        };
        let live = project_current_session_snapshot(&[LogEntry::AnnotatedToolResult {
            ts: 2,
            entry: tool_result.clone(),
        }]);
        let log = vec![LogEntry::AnnotatedSegmentStart {
            ts: 1,
            session_id,
            system_prompt: None,
            config: RequestConfig::default(),
            history: vec![tool_result],
            forked_from: None,
            compacted_from: None,
        }];
        let (_root, aggregate_root) =
            persist_history_fixture(worker_name, session_id, segment_id, vec![(segment_id, log)]);

        let retained = read_retained_session_snapshot(
            &aggregate_root,
            worker_name,
            DEFAULT_RETAINED_SNAPSHOT_MAX_BYTES,
        )
        .unwrap();
        let SessionSnapshotEntryData::ToolResult { attachments, .. } =
            &retained.snapshot.entries[0].data
        else {
            panic!("expected tool result");
        };
        assert_eq!(attachments.len(), 4);
        let SessionSnapshotEntryData::ToolResult {
            attachments: live_attachments,
            ..
        } = &live.entries[0].data
        else {
            panic!("expected live tool result");
        };
        assert_eq!(live_attachments, attachments);
        let snapshot_json = serde_json::to_vec(&retained.snapshot).unwrap();
        assert!(snapshot_json.len() < 16 * 1024);
        assert!(!String::from_utf8_lossy(&snapshot_json).contains("data_base64"));

        for (index, attachment) in attachments.iter().enumerate() {
            assert_eq!(attachment.byte_len, sizes[index] as u64);
            assert_eq!(attachment.attachment_id.len(), 43);
            let body = read_retained_session_attachment(
                &aggregate_root,
                worker_name,
                &session_id.to_string(),
                &attachment.attachment_id,
                DEFAULT_RETAINED_HISTORY_MAX_SCAN_BYTES,
                10 * 1024 * 1024,
            )
            .unwrap();
            assert_eq!(body.media_type, "image/png");
            assert_eq!(body.data.len(), sizes[index]);
            assert!(body.data.iter().all(|byte| *byte == index as u8 + 1));
        }
        assert_eq!(
            read_retained_session_attachment(
                &aggregate_root,
                worker_name,
                &crate::new_session_id().to_string(),
                &attachments[0].attachment_id,
                DEFAULT_RETAINED_HISTORY_MAX_SCAN_BYTES,
                10 * 1024 * 1024,
            )
            .unwrap_err(),
            RetainedAttachmentReadError::SessionMismatch
        );
        assert_eq!(
            read_retained_session_attachment(
                &aggregate_root,
                worker_name,
                &session_id.to_string(),
                "../../metadata.json",
                DEFAULT_RETAINED_HISTORY_MAX_SCAN_BYTES,
                10 * 1024 * 1024,
            )
            .unwrap_err(),
            RetainedAttachmentReadError::NotFound
        );
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
