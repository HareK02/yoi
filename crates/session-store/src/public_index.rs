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

use crate::worker_session_store::RetainedLogRecord;
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
    #[error("Worker Session public index cursor is stale")]
    StaleCursor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPublicIndexScanPosition {
    pub segment_index: usize,
    pub byte_offset: u64,
    pub entry_offset: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lineage: Option<SessionPublicIndexLineage>,
}

impl Default for SessionPublicIndexScanPosition {
    fn default() -> Self {
        Self {
            segment_index: 0,
            byte_offset: 0,
            entry_offset: 0,
            lineage: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPublicIndexPageEntry {
    pub segment_id: String,
    pub lineage: SessionPublicIndexLineage,
    pub entry: SessionPublicIndexEntry,
    pub scan_from: SessionPublicIndexScanPosition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPublicIndexPage {
    pub session_id: String,
    pub generation: String,
    pub entries: Vec<SessionPublicIndexPageEntry>,
    pub next_position: Option<SessionPublicIndexScanPosition>,
    pub has_more: bool,
    pub scanned_bytes: u64,
    pub scanned_segments: usize,
    pub scanned_entries: usize,
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
    pub compact_text_truncated: bool,
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

/// Incrementally scan the canonical public Session index.
///
/// Continuations stop only at a retained log-unit boundary. Ordinary records are
/// independent units; a run from `Invoke`/`RunResumed` through its terminal
/// record remains one unit so uncommitted output never crosses the public
/// projection. A single unit that cannot fit the caller's byte or entry budget
/// is therefore the only scan shape that returns [`SessionPublicIndexReadError::ResourceLimit`].
pub fn read_session_public_index_page<F>(
    session_root: &Path,
    limits: SessionPublicIndexLimits,
    expected_generation: Option<&str>,
    position: Option<SessionPublicIndexScanPosition>,
    max_results: usize,
    mut matches: F,
) -> Result<SessionPublicIndexPage, SessionPublicIndexReadError>
where
    F: FnMut(&SessionPublicIndexEntry) -> bool,
{
    if limits.max_bytes == 0
        || limits.max_segments == 0
        || limits.max_entries == 0
        || max_results == 0
    {
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
    let generation = scan_generation(&store, session_id, &segment_ids)?;
    if expected_generation.is_some_and(|expected| expected != generation) {
        return Err(SessionPublicIndexReadError::StaleCursor);
    }

    let mut position = position.unwrap_or_default();
    if position.segment_index > segment_ids.len()
        || (position.segment_index == segment_ids.len()
            && (position.byte_offset != 0
                || position.entry_offset != 0
                || position.lineage.is_some()))
    {
        return Err(SessionPublicIndexReadError::StaleCursor);
    }

    let mut scanned_bytes = 0_u64;
    let mut scanned_segments = 0_usize;
    let mut scanned_entries = 0_usize;
    let mut output = Vec::new();
    let mut made_progress = false;
    let mut stopped = false;

    while position.segment_index < segment_ids.len() && output.len() < max_results {
        if scanned_segments == limits.max_segments {
            stopped = true;
            break;
        }
        let segment_id = segment_ids[position.segment_index];
        scanned_segments += 1;

        let (file_len, lineage, mut first_for_unit) = if position.byte_offset == 0 {
            let remaining = limits.max_bytes.saturating_sub(scanned_bytes);
            let (first, first_bytes, file_len) = match store
                .read_first_log_record_read_only_bounded(session_id, segment_id, remaining)
            {
                Ok(value) => value,
                Err(StoreError::ReadLimitExceeded) if made_progress => {
                    stopped = true;
                    break;
                }
                Err(error) => return Err(map_store_error(error)),
            };
            let Some(first) = first else {
                return Err(SessionPublicIndexReadError::Corrupt);
            };
            scanned_bytes = scanned_bytes
                .checked_add(first_bytes)
                .ok_or(SessionPublicIndexReadError::ResourceLimit)?;
            let (lineage, origin) =
                validate_segment_start(session_id, segment_id, std::slice::from_ref(&first.entry))?;
            if position
                .lineage
                .as_ref()
                .is_some_and(|expected| expected != &lineage)
            {
                return Err(SessionPublicIndexReadError::StaleCursor);
            }
            validate_incremental_origin(segment_id, origin.as_ref(), &segment_ids)?;
            (file_len, lineage, Some(first))
        } else {
            let lineage = position
                .lineage
                .clone()
                .ok_or(SessionPublicIndexReadError::StaleCursor)?;
            validate_incremental_lineage(segment_id, &lineage, &segment_ids)?;
            let (file_len, _) = store
                .segment_log_observation(segment_id)
                .map_err(map_store_error)?;
            (file_len, lineage, None)
        };

        if position.byte_offset > file_len
            || (position.byte_offset == file_len && position.entry_offset != 0)
        {
            return Err(SessionPublicIndexReadError::StaleCursor);
        }
        if position.byte_offset == file_len {
            position = next_segment_position(position.segment_index);
            made_progress = true;
            continue;
        }
        loop {
            if position.byte_offset >= file_len || output.len() == max_results {
                break;
            }
            let unit_start = position.byte_offset;
            let budget_before_unit = (scanned_bytes, scanned_entries);
            let unit = match read_public_unit(
                &store,
                session_id,
                segment_id,
                unit_start,
                limits.max_bytes.saturating_sub(scanned_bytes),
                first_for_unit.take(),
            ) {
                Ok(unit) => unit,
                Err(SessionPublicIndexReadError::ResourceLimit) if made_progress => {
                    scanned_bytes = budget_before_unit.0;
                    scanned_entries = budget_before_unit.1;
                    stopped = true;
                    break;
                }
                Err(error) => return Err(error),
            };
            if unit_start > 0
                && unit
                    .entries
                    .iter()
                    .any(|entry| matches!(entry, LogEntry::AnnotatedSegmentStart { .. }))
            {
                return Err(SessionPublicIndexReadError::Corrupt);
            }
            let unit_entries = persisted_entry_units(&unit.entries);
            if scanned_entries.saturating_add(unit_entries) > limits.max_entries {
                if made_progress {
                    stopped = true;
                    break;
                }
                return Err(SessionPublicIndexReadError::ResourceLimit);
            }
            scanned_bytes = scanned_bytes
                .checked_add(unit.bytes)
                .ok_or(SessionPublicIndexReadError::ResourceLimit)?;
            scanned_entries = scanned_entries
                .checked_add(unit_entries)
                .ok_or(SessionPublicIndexReadError::ResourceLimit)?;

            let committed = committed_conversation_records(&unit.entries);
            let snapshot = crate::public_snapshot::project_session_snapshot_for_segment(
                session_id,
                Some(segment_id),
                &committed,
            );
            let projected = project_index_entries(segment_id, snapshot.entries)?;
            if position.entry_offset > projected.len()
                || (position.entry_offset != 0 && position.entry_offset == projected.len())
            {
                return Err(SessionPublicIndexReadError::StaleCursor);
            }
            for (entry_index, entry) in projected
                .into_iter()
                .enumerate()
                .skip(position.entry_offset)
            {
                let resume_at = if entry_index + 1 < unit.public_entry_count {
                    SessionPublicIndexScanPosition {
                        segment_index: position.segment_index,
                        byte_offset: unit_start,
                        entry_offset: entry_index + 1,
                        lineage: Some(lineage.clone()),
                    }
                } else if unit.end_offset == file_len {
                    next_segment_position(position.segment_index)
                } else {
                    SessionPublicIndexScanPosition {
                        segment_index: position.segment_index,
                        byte_offset: unit.end_offset,
                        entry_offset: 0,
                        lineage: Some(lineage.clone()),
                    }
                };
                if matches(&entry) {
                    output.push(SessionPublicIndexPageEntry {
                        segment_id: segment_id.to_string(),
                        lineage: lineage.clone(),
                        entry,
                        scan_from: SessionPublicIndexScanPosition {
                            segment_index: position.segment_index,
                            byte_offset: unit_start,
                            entry_offset: entry_index,
                            lineage: Some(lineage.clone()),
                        },
                    });
                    if output.len() == max_results {
                        position = resume_at;
                        break;
                    }
                }
            }
            made_progress = true;
            if output.len() == max_results {
                break;
            }
            position = if unit.end_offset == file_len {
                next_segment_position(position.segment_index)
            } else {
                SessionPublicIndexScanPosition {
                    segment_index: position.segment_index,
                    byte_offset: unit.end_offset,
                    entry_offset: 0,
                    lineage: Some(lineage.clone()),
                }
            };
            if position.segment_index >= segment_ids.len()
                || position.segment_index
                    != segment_ids
                        .iter()
                        .position(|candidate| *candidate == segment_id)
                        .unwrap_or(position.segment_index)
            {
                break;
            }
        }
        if stopped || output.len() == max_results {
            break;
        }
        if position.segment_index < segment_ids.len()
            && segment_ids[position.segment_index] == segment_id
        {
            position = next_segment_position(position.segment_index);
        }
    }

    let final_segment_ids = store
        .list_segments_read_only(session_id)
        .map_err(map_store_error)?;
    let final_generation = scan_generation(&store, session_id, &final_segment_ids)?;
    if final_generation != generation {
        return Err(SessionPublicIndexReadError::StaleCursor);
    }

    let has_more = stopped || position.segment_index < segment_ids.len();
    Ok(SessionPublicIndexPage {
        session_id: session_id.to_string(),
        generation,
        entries: output,
        next_position: has_more.then_some(position),
        has_more,
        scanned_bytes,
        scanned_segments,
        scanned_entries,
    })
}

#[derive(Debug)]
struct PublicLogUnit {
    entries: Vec<LogEntry>,
    end_offset: u64,
    bytes: u64,
    public_entry_count: usize,
}

fn read_public_unit(
    store: &WorkerSessionStore,
    session_id: SessionId,
    segment_id: SegmentId,
    start_offset: u64,
    max_bytes: u64,
    first: Option<RetainedLogRecord>,
) -> Result<PublicLogUnit, SessionPublicIndexReadError> {
    let (first, first_bytes) = match first {
        Some(record) => (Some(record), 0),
        None => store
            .read_next_log_record_read_only_bounded(session_id, segment_id, start_offset, max_bytes)
            .map_err(map_store_error)?,
    };
    let Some(first) = first else {
        return Err(SessionPublicIndexReadError::StaleCursor);
    };
    if first.start_offset != start_offset {
        return Err(SessionPublicIndexReadError::StaleCursor);
    }
    let mut bytes = first_bytes;
    let mut end_offset = first.end_offset;
    let run = matches!(
        first.entry,
        LogEntry::Invoke { .. } | LogEntry::RunResumed { .. }
    );
    let mut entries = vec![first.entry];
    if run {
        loop {
            let (next, read_bytes) = store
                .read_next_log_record_read_only_bounded(
                    session_id,
                    segment_id,
                    end_offset,
                    max_bytes.saturating_sub(bytes),
                )
                .map_err(map_store_error)?;
            let Some(next) = next else {
                break;
            };
            bytes = bytes
                .checked_add(read_bytes)
                .ok_or(SessionPublicIndexReadError::ResourceLimit)?;
            end_offset = next.end_offset;
            let terminal = matches!(
                next.entry,
                LogEntry::RunCompleted { .. }
                    | LogEntry::RunYielded { .. }
                    | LogEntry::RunCancelled { .. }
                    | LogEntry::RunErrored { .. }
                    | LogEntry::PausedTurnAbandoned { .. }
            );
            entries.push(next.entry);
            if terminal {
                break;
            }
        }
    }
    let committed = committed_conversation_records(&entries);
    let snapshot = crate::public_snapshot::project_session_snapshot_for_segment(
        session_id,
        Some(segment_id),
        &committed,
    );
    let public_entry_count = project_index_entries(segment_id, snapshot.entries)?.len();
    Ok(PublicLogUnit {
        entries,
        end_offset,
        bytes,
        public_entry_count,
    })
}

fn next_segment_position(segment_index: usize) -> SessionPublicIndexScanPosition {
    SessionPublicIndexScanPosition {
        segment_index: segment_index.saturating_add(1),
        byte_offset: 0,
        entry_offset: 0,
        lineage: None,
    }
}

fn validate_incremental_lineage(
    segment_id: SegmentId,
    lineage: &SessionPublicIndexLineage,
    segment_ids: &[SegmentId],
) -> Result<(), SessionPublicIndexReadError> {
    let parent = match lineage.origin_kind {
        SessionPublicIndexOriginKind::Root => {
            if lineage.parent_segment_id.is_some() || lineage.parent_turn_index.is_some() {
                return Err(SessionPublicIndexReadError::StaleCursor);
            }
            return Ok(());
        }
        SessionPublicIndexOriginKind::Fork | SessionPublicIndexOriginKind::Compact => lineage
            .parent_segment_id
            .as_deref()
            .and_then(|value| uuid::Uuid::parse_str(value).ok())
            .ok_or(SessionPublicIndexReadError::StaleCursor)?,
    };
    if parent == segment_id || !segment_ids.contains(&parent) || lineage.parent_turn_index.is_none()
    {
        return Err(SessionPublicIndexReadError::StaleCursor);
    }
    Ok(())
}

fn validate_incremental_origin(
    segment_id: SegmentId,
    origin: Option<&SegmentOrigin>,
    segment_ids: &[SegmentId],
) -> Result<(), SessionPublicIndexReadError> {
    if origin.is_some_and(|origin| {
        origin.segment_id == segment_id || !segment_ids.contains(&origin.segment_id)
    }) {
        return Err(SessionPublicIndexReadError::Corrupt);
    }
    Ok(())
}

fn scan_generation(
    store: &WorkerSessionStore,
    session_id: SessionId,
    segment_ids: &[SegmentId],
) -> Result<String, SessionPublicIndexReadError> {
    let mut generation = Sha256::new();
    generation.update(b"yoi-session-public-index-scan-v1\0");
    generation.update(session_id.as_bytes());
    for segment_id in segment_ids {
        let (len, modified_nanos) = store
            .segment_log_observation(*segment_id)
            .map_err(map_store_error)?;
        generation.update(segment_id.as_bytes());
        generation.update(len.to_be_bytes());
        generation.update(modified_nanos.to_be_bytes());
    }
    Ok(URL_SAFE_NO_PAD.encode(generation.finalize()))
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

        let (compact_text, compact_text_truncated) = bounded_compact_text(&compact_source);
        projected.push(SessionPublicIndexEntry {
            segment_id: segment_id.to_string(),
            entry_ref,
            kind,
            provenance: entry.provenance,
            timestamp: entry.timestamp,
            tool_part,
            tool_name,
            compact_text,
            compact_text_truncated,
            full_text,
        });
    }

    Ok(projected)
}

fn bounded_compact_text(value: &str) -> (String, bool) {
    (
        truncate_utf8(value, SESSION_PUBLIC_INDEX_COMPACT_TEXT_MAX_BYTES),
        value.len() > SESSION_PUBLIC_INDEX_COMPACT_TEXT_MAX_BYTES,
    )
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
    fn compact_text_reports_utf8_safe_truncation() {
        let exact = "x".repeat(SESSION_PUBLIC_INDEX_COMPACT_TEXT_MAX_BYTES);
        assert_eq!(bounded_compact_text(&exact), (exact, false));

        let oversized = "界".repeat(SESSION_PUBLIC_INDEX_COMPACT_TEXT_MAX_BYTES);
        let (compact, truncated) = bounded_compact_text(&oversized);
        assert!(truncated);
        assert!(compact.len() <= SESSION_PUBLIC_INDEX_COMPACT_TEXT_MAX_BYTES);
        assert!(compact.ends_with('…'));
        assert!(compact.is_char_boundary(compact.len()));
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
    fn incremental_scan_finds_hit_beyond_segment_budget_and_stales_after_append() {
        let (_temp, root, store) = create_store();
        let session_id = crate::new_session_id();
        for index in 0..65_u128 {
            let text = if index == 64 {
                "needle beyond first page"
            } else {
                "ordinary entry"
            };
            store
                .create_segment(
                    session_id,
                    segment_id(1_000 + index),
                    &[start(
                        session_id,
                        vec![message(&format!("entry-{index}"), LoggedRole::User, text)],
                        None,
                        None,
                    )],
                )
                .unwrap();
        }
        let limits = SessionPublicIndexLimits {
            max_segments: 64,
            ..SessionPublicIndexLimits::default()
        };
        let first = read_session_public_index_page(&root, limits, None, None, 10, |entry| {
            entry.full_text.contains("needle")
        })
        .unwrap();
        assert!(first.entries.is_empty());
        assert!(first.has_more);
        assert_eq!(first.scanned_segments, 64);
        let next_position = first.next_position.clone().unwrap();

        let second = read_session_public_index_page(
            &root,
            limits,
            Some(&first.generation),
            Some(next_position.clone()),
            10,
            |entry| entry.full_text.contains("needle"),
        )
        .unwrap();
        assert_eq!(second.entries.len(), 1);
        assert_eq!(
            second.entries[0].entry.full_text,
            "needle beyond first page"
        );
        assert!(!second.has_more);
        assert!(second.next_position.is_none());

        store
            .append(
                session_id,
                segment_id(1_064),
                &LogEntry::AnnotatedAssistantItem {
                    ts: 2,
                    entry: message("mutated", LoggedRole::Assistant, "changed"),
                },
            )
            .unwrap();
        assert_eq!(
            read_session_public_index_page(
                &root,
                limits,
                Some(&first.generation),
                Some(next_position),
                10,
                |_| true,
            ),
            Err(SessionPublicIndexReadError::StaleCursor)
        );
    }

    #[test]
    fn incremental_scan_resumes_across_byte_and_entry_budgets() {
        let (_temp, root, store) = create_store();
        let session_id = crate::new_session_id();
        let first_segment = segment_id(1_500);
        let second_segment = segment_id(1_501);
        for (segment_id, id, text) in [
            (first_segment, "entry-a", "aaaaaa"),
            (second_segment, "entry-b", "needle"),
        ] {
            store
                .create_segment(
                    session_id,
                    segment_id,
                    &[start(
                        session_id,
                        vec![message(id, LoggedRole::User, text)],
                        None,
                        None,
                    )],
                )
                .unwrap();
        }
        let first_len = store.segment_log_len(first_segment).unwrap();
        for limits in [
            SessionPublicIndexLimits {
                max_bytes: first_len,
                ..SessionPublicIndexLimits::default()
            },
            SessionPublicIndexLimits {
                max_entries: 2,
                ..SessionPublicIndexLimits::default()
            },
        ] {
            let first = read_session_public_index_page(&root, limits, None, None, 10, |entry| {
                entry.full_text == "needle"
            })
            .unwrap();
            assert!(first.entries.is_empty());
            assert!(first.has_more);
            let second = read_session_public_index_page(
                &root,
                limits,
                Some(&first.generation),
                first.next_position,
                10,
                |entry| entry.full_text == "needle",
            )
            .unwrap();
            assert_eq!(second.entries.len(), 1);
            assert!(!second.has_more);
        }
    }

    #[test]
    fn incremental_scan_rejects_one_indivisible_record_over_the_byte_budget() {
        let (_temp, root, store) = create_store();
        let session_id = crate::new_session_id();
        store
            .create_segment(
                session_id,
                segment_id(2_000),
                &[start(
                    session_id,
                    vec![message("large-entry", LoggedRole::User, &"x".repeat(8_192))],
                    None,
                    None,
                )],
            )
            .unwrap();

        assert_eq!(
            read_session_public_index_page(
                &root,
                SessionPublicIndexLimits {
                    max_bytes: 512,
                    ..SessionPublicIndexLimits::default()
                },
                None,
                None,
                10,
                |_| true,
            ),
            Err(SessionPublicIndexReadError::ResourceLimit)
        );
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
