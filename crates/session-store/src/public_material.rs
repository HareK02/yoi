//! Original public material from a saved, rollback-ineligible Session prefix.
//!
//! Unlike the Memory evidence index, this reader includes saved output from
//! interrupted runs and active runs with materialized AI activity. It does not
//! interpret domain-specific recording or completion policy. Callers authorize
//! the Session root and explicitly select its active Segment.
//!
//! Catalogs retain only descriptors and record locations, never original bodies.
//! Records are decoded one at a time under an explicit allocation ceiling, not
//! under a whole-run scan budget. Canonical bodies are streamed to SHA-256 and
//! bounded UTF-8 windows. A single JSONL record (including a Segment seed) must
//! fit `max_record_bytes`; exceeding that ceiling is an error, not a preview.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    LogEntry, LoggedHistoryEntry, LoggedItem, LoggedRole, LoggedSessionHistoryOrigin, SegmentId,
    SegmentOrigin, SessionId, WorkerSessionStore,
};

#[derive(Debug, thiserror::Error)]
pub enum PublicMaterialError {
    #[error("public material storage: {0}")]
    Storage(#[from] io::Error),
    #[error("public material record: {0}")]
    Record(#[from] serde_json::Error),
    #[error("public material source is missing")]
    Missing,
    #[error("public material source was permanently erased")]
    Erased,
    #[error("public material source requires migration or is unavailable")]
    Unavailable,
    #[error("public material lineage or identity is inconsistent")]
    Inconsistent,
    #[error("public material resource ceiling exceeded")]
    ResourceLimit,
    #[error("public material changed since the catalog was read")]
    Changed,
    #[error("public material offset is not a UTF-8 boundary or is outside the body")]
    InvalidOffset,
}

#[derive(Debug, Clone, Copy)]
pub struct PublicMaterialLimits {
    pub max_segments: usize,
    /// Total history identities examined, including inherited and private items.
    pub max_entries: usize,
    /// Allocation ceiling for one persisted JSONL record, not an entire run.
    pub max_record_bytes: u64,
}

impl Default for PublicMaterialLimits {
    fn default() -> Self {
        Self {
            max_segments: 256,
            max_entries: 1_000_000,
            max_record_bytes: 64 * 1024 * 1024,
        }
    }
}

/// Original identity, never a body-based deduplication key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PublicMaterialIdentity {
    pub segment_id: SegmentId,
    pub entry_ref: String,
}

/// Public provenance distinguishes saved conversation from Host control facts. Run boundaries
/// are not model output, user intent, or evidence that the requested work succeeded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PublicMaterialOrigin {
    History { origin: LoggedSessionHistoryOrigin },
    HostRunBoundary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicRunOutcome {
    Finished,
    Paused,
    Yielded,
    LimitReached,
}

/// Only whitelisted execution facts cross this boundary; raw error/provider messages do not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PublicRunBoundary {
    Completed {
        outcome: PublicRunOutcome,
        interrupted: bool,
    },
    Yielded,
    Resumed,
    Cancelled,
    Errored,
    Abandoned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicMaterialDescriptor {
    pub identity: PublicMaterialIdentity,
    /// Zero-based position on this explicitly selected active lineage. Forks
    /// may remove a suffix; callers must reconcile identities, not assume that
    /// positions from a previous active lineage are interchangeable.
    pub canonical_order: u64,
    pub origin: PublicMaterialOrigin,
    /// Lowercase hexadecimal SHA-256 of the complete canonical UTF-8 body.
    pub sha256: String,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicMaterialContent {
    pub content: String,
    pub next_offset: u64,
    pub total_bytes: u64,
    pub sha256: String,
}

#[derive(Clone)]
struct Location {
    segment: SegmentId,
    offset: u64,
    history_index: usize,
}

struct Material {
    descriptor: PublicMaterialDescriptor,
    location: Location,
    is_tool_result: bool,
}

struct SegmentHeader {
    id: SegmentId,
    parent: Option<SegmentOrigin>,
    compact: bool,
    // Compaction's exact source-history IDs disambiguate mid-turn boundaries.
    source_ids: Option<Vec<String>>,
    file_len: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicMaterialUse {
    Recording,
    MemoryEvidence,
}

/// A fixed material catalog. Opening a new catalog observes newly appended
/// records; reading a body through this catalog verifies its saved digest and
/// byte length again. Missing/corrected/deleted sources are explicit failures.
/// Evidence eligibility never publishes private model text or Feature payload:
/// System inputs also require an explicit public projection for every public use.
pub struct SessionPublicMaterialReader {
    session_id: SessionId,
    active_segment_id: SegmentId,
    layout: Layout,
    limits: PublicMaterialLimits,
    materials: Vec<Material>,
    purpose: PublicMaterialUse,
    retention: Option<crate::retention::SessionRetentionStore>,
}

enum Layout {
    Worker(PathBuf),
    Fs(PathBuf),
}

impl Layout {
    fn path(&self, segment: SegmentId) -> PathBuf {
        match self {
            Self::Worker(root) => root.join("segments").join(format!("{segment}.jsonl")),
            Self::Fs(root) => root.join(format!("{segment}.jsonl")),
        }
    }
}

impl SessionPublicMaterialReader {
    pub fn open_worker(
        session_root: &Path,
        active_segment_id: SegmentId,
        limits: PublicMaterialLimits,
    ) -> Result<Self, PublicMaterialError> {
        Self::open_worker_guarded(
            session_root,
            active_segment_id,
            limits,
            None,
            PublicMaterialUse::Recording,
        )
    }

    /// Uses the Host's aggregate-external control root for the whole scan and every body read.
    /// The same store must fence publishers, archive transfer, and physical deletion.
    pub fn open_worker_with_retention(
        session_root: &Path,
        active_segment_id: SegmentId,
        limits: PublicMaterialLimits,
        retention: crate::retention::SessionRetentionStore,
    ) -> Result<Self, PublicMaterialError> {
        Self::open_worker_for_use(
            session_root,
            active_segment_id,
            limits,
            retention,
            PublicMaterialUse::Recording,
        )
    }

    /// Selects the evidence purpose independently of public display and observation.
    pub fn open_worker_for_use(
        session_root: &Path,
        active_segment_id: SegmentId,
        limits: PublicMaterialLimits,
        retention: crate::retention::SessionRetentionStore,
        purpose: PublicMaterialUse,
    ) -> Result<Self, PublicMaterialError> {
        Self::open_worker_guarded(
            session_root,
            active_segment_id,
            limits,
            Some(retention),
            purpose,
        )
    }

    fn open_worker_guarded(
        session_root: &Path,
        active_segment_id: SegmentId,
        limits: PublicMaterialLimits,
        retention: Option<crate::retention::SessionRetentionStore>,
        purpose: PublicMaterialUse,
    ) -> Result<Self, PublicMaterialError> {
        let store = WorkerSessionStore::open_read_only(session_root)
            .map_err(|_| PublicMaterialError::Unavailable)?;
        let session_id = store
            .session_id()
            .map_err(|_| PublicMaterialError::Unavailable)?
            .ok_or(PublicMaterialError::Missing)?;
        Self::open(
            session_id,
            active_segment_id,
            Layout::Worker(session_root.into()),
            limits,
            retention,
            purpose,
        )
    }

    pub fn open_fs(
        sessions_root: &Path,
        session_id: SessionId,
        active_segment_id: SegmentId,
        limits: PublicMaterialLimits,
    ) -> Result<Self, PublicMaterialError> {
        Self::open_fs_with_retention(
            sessions_root,
            session_id,
            active_segment_id,
            limits,
            crate::retention::fs_retention(sessions_root),
        )
    }

    /// Keeps the same erasure fence when a Host relocates originals to retained storage.
    pub fn open_fs_with_retention(
        sessions_root: &Path,
        session_id: SessionId,
        active_segment_id: SegmentId,
        limits: PublicMaterialLimits,
        retention: crate::retention::SessionRetentionStore,
    ) -> Result<Self, PublicMaterialError> {
        Self::open_fs_for_use(
            sessions_root,
            session_id,
            active_segment_id,
            limits,
            retention,
            PublicMaterialUse::Recording,
        )
    }

    /// Uses retained originals with the independently selected evidence policy.
    pub fn open_fs_for_use(
        sessions_root: &Path,
        session_id: SessionId,
        active_segment_id: SegmentId,
        limits: PublicMaterialLimits,
        retention: crate::retention::SessionRetentionStore,
        purpose: PublicMaterialUse,
    ) -> Result<Self, PublicMaterialError> {
        Self::open(
            session_id,
            active_segment_id,
            Layout::Fs(sessions_root.join(session_id.to_string())),
            limits,
            Some(retention),
            purpose,
        )
    }

    pub fn session_id(&self) -> SessionId {
        self.session_id
    }
    pub fn active_segment_id(&self) -> SegmentId {
        self.active_segment_id
    }
    pub fn descriptors(&self) -> impl ExactSizeIterator<Item = &PublicMaterialDescriptor> {
        self.materials.iter().map(|value| &value.descriptor)
    }

    fn open(
        session_id: SessionId,
        active_segment_id: SegmentId,
        layout: Layout,
        limits: PublicMaterialLimits,
        retention: Option<crate::retention::SessionRetentionStore>,
        purpose: PublicMaterialUse,
    ) -> Result<Self, PublicMaterialError> {
        let _guard = retention
            .as_ref()
            .map(|store| store.lock_live(session_id).map_err(retention_error))
            .transpose()?;
        if limits.max_segments == 0 || limits.max_entries == 0 || limits.max_record_bytes == 0 {
            return Err(PublicMaterialError::ResourceLimit);
        }
        let mut chain = Vec::new();
        let mut visited = HashSet::new();
        let mut current = active_segment_id;
        let mut header_entries = 0usize;
        loop {
            if !visited.insert(current) {
                return Err(PublicMaterialError::Inconsistent);
            }
            if visited.len() > limits.max_segments {
                return Err(PublicMaterialError::ResourceLimit);
            }
            let mut reader = RecordReader::open(&layout.path(current), limits.max_record_bytes)?;
            let file_len = reader.end;
            let Some((
                _,
                LogEntry::AnnotatedSegmentStart {
                    session_id: stored,
                    forked_from,
                    compacted_from,
                    history,
                    ..
                },
            )) = reader.next()?
            else {
                return Err(PublicMaterialError::Inconsistent);
            };
            if stored != session_id || (forked_from.is_some() && compacted_from.is_some()) {
                return Err(PublicMaterialError::Inconsistent);
            }
            header_entries = header_entries
                .checked_add(history.len())
                .ok_or(PublicMaterialError::ResourceLimit)?;
            if header_entries > limits.max_entries {
                return Err(PublicMaterialError::ResourceLimit);
            }
            let compact = compacted_from.is_some();
            let source_ids: Option<Vec<String>> = if compact {
                let summaries: Vec<_> = history
                    .iter()
                    .filter(|e| {
                        matches!(
                            e.metadata.origin,
                            LoggedSessionHistoryOrigin::CompactionSummary
                        )
                    })
                    .collect();
                if summaries.len() != 1 {
                    return Err(PublicMaterialError::Inconsistent);
                }
                Some(
                    summaries[0]
                        .metadata
                        .derivation
                        .as_ref()
                        .ok_or(PublicMaterialError::Inconsistent)?
                        .sources
                        .iter()
                        .map(|id| id.0.clone())
                        .collect(),
                )
            } else {
                None
            };
            header_entries = header_entries
                .checked_add(source_ids.as_ref().map_or(0, |ids| ids.len()))
                .ok_or(PublicMaterialError::ResourceLimit)?;
            if header_entries > limits.max_entries {
                return Err(PublicMaterialError::ResourceLimit);
            }
            let parent = forked_from.or(compacted_from);
            let next = parent.as_ref().map(|p| p.segment_id);
            chain.push(SegmentHeader {
                id: current,
                parent,
                compact,
                source_ids,
                file_len,
            });
            match next {
                Some(id) => current = id,
                None => break,
            }
        }
        chain.reverse();
        let mut materials = Vec::<Material>::new();
        let mut identities = HashMap::<String, usize>::new();
        let mut examined = 0usize;
        let mut expected_fork_seed: Option<Vec<String>> = None;
        for (segment_index, header) in chain.iter().enumerate() {
            let child = chain.get(segment_index + 1);
            let mut reader = RecordReader::open(&layout.path(header.id), limits.max_record_bytes)?;
            reader.end = header.file_len;
            let mut history_ids = Vec::<String>::new();
            let mut history_seen = HashSet::<String>::new();
            let mut inherited_order = None;
            let mut pending = Vec::<Material>::new();
            let mut in_run = false;
            let mut safe = true;
            let mut turn = 0usize;
            let mut reached_cut = child.is_none();
            let mut compact_cut_turn_seen = false;
            while let Some((offset, record)) = reader.next()? {
                // Once a compaction's exact source history is reached, consume
                // only non-history boundaries/corrections. A later history
                // addition belongs to the parent's unadopted suffix.
                if reached_cut
                    && child.is_some_and(|c| c.compact)
                    && matches!(
                        record,
                        LogEntry::AnnotatedUserInput { .. }
                            | LogEntry::AnnotatedAssistantItem { .. }
                            | LogEntry::AnnotatedToolResult { .. }
                            | LogEntry::AnnotatedSystemItem { .. }
                    )
                {
                    break;
                }
                let seed = matches!(record, LogEntry::AnnotatedSegmentStart { .. });
                if seed {
                    if let LogEntry::AnnotatedSegmentStart {
                        history,
                        session_id: stored,
                        ..
                    } = &record
                    {
                        if *stored != session_id {
                            return Err(PublicMaterialError::Inconsistent);
                        }
                        if let Some(expected) = expected_fork_seed.take() {
                            if history
                                .iter()
                                .map(|e| &e.metadata.entry_id.0)
                                .ne(expected.iter())
                            {
                                return Err(PublicMaterialError::Inconsistent);
                            }
                        }
                    }
                }
                if offset != 0 && seed {
                    return Err(PublicMaterialError::Inconsistent);
                }
                match &record {
                    LogEntry::Invoke { .. } | LogEntry::RunResumed { .. } => {
                        if !pending.is_empty() {
                            return Err(PublicMaterialError::Inconsistent);
                        }
                        in_run = true;
                        safe = false;
                    }
                    LogEntry::RunCompleted { .. }
                    | LogEntry::RunYielded { .. }
                    | LogEntry::RunCancelled { .. }
                    | LogEntry::RunErrored { .. }
                    | LogEntry::PausedTurnAbandoned { .. } => {
                        safe = true;
                        in_run = false;
                    }
                    LogEntry::TurnEnd { turn_count, .. } => turn = *turn_count,
                    LogEntry::ActiveRunCheckpoint {
                        total_turn_count, ..
                    } => turn = *total_turn_count,
                    _ => {}
                }
                let system_history;
                let entries: &[LoggedHistoryEntry] = match &record {
                    LogEntry::AnnotatedSegmentStart { history, .. }
                    | LogEntry::AnnotatedUserInput { history, .. } => history,
                    LogEntry::AnnotatedAssistantItem { entry, .. }
                    | LogEntry::AnnotatedToolResult { entry, .. }
                    | LogEntry::ToolResultCorrected { entry, .. } => std::slice::from_ref(entry),
                    LogEntry::AnnotatedSystemItem { entry, .. } => {
                        system_history = entry.to_history_entry();
                        std::slice::from_ref(&system_history)
                    }
                    _ => &[],
                };
                for (history_index, entry) in entries.iter().enumerate() {
                    charge(&mut examined, limits.max_entries)?;
                    let id = &entry.metadata.entry_id.0;
                    if id.is_empty() {
                        return Err(PublicMaterialError::Inconsistent);
                    }
                    let corrected = matches!(record, LogEntry::ToolResultCorrected { .. });
                    if !corrected {
                        if !history_seen.insert(id.clone()) {
                            return Err(PublicMaterialError::Inconsistent);
                        }
                        history_ids.push(id.clone());
                    }
                    if in_run && ai_materialized(&entry.item) {
                        safe = true;
                    }
                    let Some((sha256, total_bytes)) = canonical_metadata(entry, purpose)? else {
                        continue;
                    };
                    let location = Location {
                        segment: header.id,
                        offset,
                        history_index,
                    };
                    let descriptor = PublicMaterialDescriptor {
                        identity: PublicMaterialIdentity {
                            segment_id: header.id,
                            entry_ref: format!("E{id}"),
                        },
                        canonical_order: 0,
                        origin: PublicMaterialOrigin::History {
                            origin: entry.metadata.origin.clone(),
                        },
                        sha256,
                        total_bytes,
                    };
                    if seed && header.parent.is_some() {
                        // Every inherited original must already exist on the
                        // selected ancestor prefix; compare identity + bytes.
                        let index = identities
                            .get(id)
                            .ok_or(PublicMaterialError::Inconsistent)?;
                        if inherited_order.is_some_and(|previous| *index <= previous) {
                            return Err(PublicMaterialError::Inconsistent);
                        }
                        inherited_order = Some(*index);
                        let original = &materials[*index].descriptor;
                        if original.sha256 != descriptor.sha256
                            || original.total_bytes != descriptor.total_bytes
                            || original.origin != descriptor.origin
                        {
                            return Err(PublicMaterialError::Inconsistent);
                        }
                    } else if corrected {
                        let index = identities
                            .get(id)
                            .ok_or(PublicMaterialError::Inconsistent)?;
                        let original = &mut materials[*index];
                        if !original.is_tool_result
                            || original.descriptor.origin != descriptor.origin
                            || !matches!(entry.item, LoggedItem::ToolResult { .. })
                        {
                            return Err(PublicMaterialError::Inconsistent);
                        }
                        original.descriptor.sha256 = descriptor.sha256;
                        original.descriptor.total_bytes = descriptor.total_bytes;
                        original.location = location;
                    } else {
                        if identities.contains_key(id)
                            || pending.iter().any(|m| {
                                m.descriptor.identity.entry_ref == descriptor.identity.entry_ref
                            })
                        {
                            return Err(PublicMaterialError::Inconsistent);
                        }
                        pending.push(Material {
                            descriptor,
                            location,
                            is_tool_result: matches!(entry.item, LoggedItem::ToolResult { .. }),
                        });
                    }
                }
                if let Some(boundary) = run_boundary(&record) {
                    charge(&mut examined, limits.max_entries)?;
                    let reference = boundary_reference(&record, session_id, header.id, offset);
                    if identities.contains_key(&reference[1..])
                        || pending
                            .iter()
                            .any(|m| m.descriptor.identity.entry_ref == reference)
                    {
                        return Err(PublicMaterialError::Inconsistent);
                    }
                    let mut sink = BodySink::new(0, 0);
                    serde_json::to_writer(&mut sink, &CanonicalBody::RunBoundary { boundary })?;
                    let (sha256, total_bytes, _) = sink.finish()?;
                    pending.push(Material {
                        descriptor: PublicMaterialDescriptor {
                            identity: PublicMaterialIdentity {
                                segment_id: header.id,
                                entry_ref: reference,
                            },
                            canonical_order: 0,
                            origin: PublicMaterialOrigin::HostRunBoundary,
                            sha256,
                            total_bytes,
                        },
                        location: Location {
                            segment: header.id,
                            offset,
                            history_index: 0,
                        },
                        is_tool_result: false,
                    });
                }
                if safe {
                    flush(&mut materials, &mut identities, &mut pending);
                }
                if let Some(child) = child {
                    if child.compact {
                        let sources = child
                            .source_ids
                            .as_ref()
                            .ok_or(PublicMaterialError::Inconsistent)?;
                        if &history_ids == sources {
                            // Do not stop at an earlier non-history record when
                            // compaction's source-history is empty.
                            if !sources.is_empty() || seed {
                                reached_cut = true;
                                if child
                                    .parent
                                    .as_ref()
                                    .is_some_and(|p| p.at_turn_index == turn)
                                {
                                    compact_cut_turn_seen = true;
                                }
                            }
                        }
                    } else if (seed && child.parent.as_ref().is_some_and(|p| p.at_turn_index == 0))
                        || (turn
                            == child
                                .parent
                                .as_ref()
                                .ok_or(PublicMaterialError::Inconsistent)?
                                .at_turn_index
                            && matches!(record, LogEntry::TurnEnd { .. }))
                    {
                        reached_cut = true;
                        break;
                    }
                }
            }
            if !reached_cut || (child.is_some_and(|c| c.compact) && !compact_cut_turn_seen) {
                return Err(PublicMaterialError::Inconsistent);
            }
            if child.is_some_and(|c| !c.compact) {
                expected_fork_seed = Some(history_ids);
            }
            // A compaction seed proves its selected input is durably retained;
            // still require materialized AI or a terminal boundary for a run.
            if !pending.is_empty() && child.is_some() {
                return Err(PublicMaterialError::Inconsistent);
            }
        }
        Ok(Self {
            session_id,
            active_segment_id,
            layout,
            limits,
            materials,
            purpose,
            retention: retention.clone(),
        })
    }

    /// Read a canonical body window without allocating its full rendered text.
    /// `max_content_bytes >= 4` guarantees progress for every UTF-8 scalar.
    pub fn read_content(
        &self,
        descriptor: &PublicMaterialDescriptor,
        offset: u64,
        max_content_bytes: usize,
    ) -> Result<PublicMaterialContent, PublicMaterialError> {
        let _guard = self
            .retention
            .as_ref()
            .map(|store| store.lock_live(self.session_id).map_err(retention_error))
            .transpose()?;
        if max_content_bytes < 4 {
            return Err(PublicMaterialError::ResourceLimit);
        }
        if offset > descriptor.total_bytes {
            return Err(PublicMaterialError::InvalidOffset);
        }
        let material = self
            .materials
            .get(
                usize::try_from(descriptor.canonical_order)
                    .map_err(|_| PublicMaterialError::Changed)?,
            )
            .filter(|m| &m.descriptor == descriptor)
            .ok_or(PublicMaterialError::Changed)?;
        let mut reader = RecordReader::open(
            &self.layout.path(material.location.segment),
            self.limits.max_record_bytes,
        )?;
        reader.seek(material.location.offset)?;
        let Some((_, record)) = reader.next()? else {
            return Err(PublicMaterialError::Missing);
        };
        let system_history;
        let (body, origin) = if let Some(boundary) = run_boundary(&record) {
            if boundary_reference(
                &record,
                self.session_id,
                material.location.segment,
                material.location.offset,
            ) != descriptor.identity.entry_ref
            {
                return Err(PublicMaterialError::Changed);
            }
            (
                CanonicalBody::RunBoundary { boundary },
                PublicMaterialOrigin::HostRunBoundary,
            )
        } else {
            let entry = match &record {
                LogEntry::AnnotatedSegmentStart { history, .. }
                | LogEntry::AnnotatedUserInput { history, .. } => {
                    history.get(material.location.history_index)
                }
                LogEntry::AnnotatedAssistantItem { entry, .. }
                | LogEntry::AnnotatedToolResult { entry, .. }
                | LogEntry::ToolResultCorrected { entry, .. } => Some(entry),
                LogEntry::AnnotatedSystemItem { entry, .. } => {
                    system_history = entry.to_history_entry();
                    Some(&system_history)
                }
                _ => None,
            }
            .filter(|e| format!("E{}", e.metadata.entry_id.0) == descriptor.identity.entry_ref)
            .ok_or(PublicMaterialError::Changed)?;
            (
                canonical_body(entry, self.purpose).ok_or(PublicMaterialError::Changed)?,
                PublicMaterialOrigin::History {
                    origin: entry.metadata.origin.clone(),
                },
            )
        };
        let mut sink = BodySink::new(offset, max_content_bytes);
        serde_json::to_writer(&mut sink, &body)?;
        let (sha256, total_bytes, content) = sink.finish()?;
        if sha256 != descriptor.sha256
            || total_bytes != descriptor.total_bytes
            || origin != descriptor.origin
        {
            return Err(PublicMaterialError::Changed);
        }
        Ok(PublicMaterialContent {
            next_offset: offset + content.len() as u64,
            content,
            total_bytes,
            sha256,
        })
    }
}

fn retention_error(error: crate::StoreError) -> PublicMaterialError {
    match error {
        crate::StoreError::SessionErased(_) => PublicMaterialError::Erased,
        crate::StoreError::Io(error) => PublicMaterialError::Storage(error),
        crate::StoreError::Serde(error) => PublicMaterialError::Record(error),
        _ => PublicMaterialError::Unavailable,
    }
}

fn charge(examined: &mut usize, max: usize) -> Result<(), PublicMaterialError> {
    *examined = examined
        .checked_add(1)
        .ok_or(PublicMaterialError::ResourceLimit)?;
    if *examined > max {
        return Err(PublicMaterialError::ResourceLimit);
    }
    Ok(())
}

fn flush(
    materials: &mut Vec<Material>,
    identities: &mut HashMap<String, usize>,
    pending: &mut Vec<Material>,
) {
    for mut material in pending.drain(..) {
        let index = materials.len();
        material.descriptor.canonical_order = index as u64;
        identities.insert(material.descriptor.identity.entry_ref[1..].into(), index);
        materials.push(material);
    }
}

fn ai_materialized(item: &LoggedItem) -> bool {
    !matches!(
        item,
        LoggedItem::Message {
            role: LoggedRole::User | LoggedRole::System,
            ..
        }
    )
}

/// Versioned canonical JSON framing. This is original text, not a display
/// preview: both result summary and optional full content remain distinguishable.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CanonicalBody<'a> {
    RunBoundary {
        boundary: PublicRunBoundary,
    },
    Message {
        origin: &'a LoggedSessionHistoryOrigin,
        role: LoggedRole,
        content: std::borrow::Cow<'a, [crate::LoggedContentPart]>,
    },
    ToolCall {
        origin: &'a LoggedSessionHistoryOrigin,
        call_id: &'a str,
        name: &'a str,
        arguments: &'a str,
    },
    ToolResult {
        origin: &'a LoggedSessionHistoryOrigin,
        call_id: &'a str,
        summary: &'a str,
        content: &'a Option<String>,
        disposition: &'a agen::tool::ToolResultDisposition,
        is_error: bool,
    },
}

fn run_boundary(record: &LogEntry) -> Option<PublicRunBoundary> {
    Some(match record {
        LogEntry::RunCompleted {
            result,
            interrupted,
            ..
        } => PublicRunBoundary::Completed {
            outcome: match result {
                agen::EngineResult::Finished => PublicRunOutcome::Finished,
                agen::EngineResult::Paused => PublicRunOutcome::Paused,
                agen::EngineResult::Yielded => PublicRunOutcome::Yielded,
                agen::EngineResult::LimitReached => PublicRunOutcome::LimitReached,
            },
            interrupted: *interrupted,
        },
        LogEntry::RunYielded { .. } => PublicRunBoundary::Yielded,
        LogEntry::RunResumed { .. } => PublicRunBoundary::Resumed,
        LogEntry::RunCancelled { .. } => PublicRunBoundary::Cancelled,
        LogEntry::RunErrored { .. } => PublicRunBoundary::Errored,
        LogEntry::PausedTurnAbandoned { .. } => PublicRunBoundary::Abandoned,
        _ => return None,
    })
}

fn boundary_reference(
    record: &LogEntry,
    session: SessionId,
    segment: SegmentId,
    offset: u64,
) -> String {
    let id = match record {
        LogEntry::RunCompleted { entry_id, .. }
        | LogEntry::PausedTurnAbandoned { entry_id, .. }
        | LogEntry::RunYielded { entry_id, .. }
        | LogEntry::RunResumed { entry_id, .. }
        | LogEntry::RunCancelled { entry_id, .. }
        | LogEntry::RunErrored { entry_id, .. } => entry_id.as_ref(),
        _ => None,
    };
    // Reuse the storage convention for legacy transitions, including the exact physical
    // record position. New normal completions have no persisted transition ID either.
    id.map(|id| format!("E{}", id.0))
        .unwrap_or_else(|| format!("El-run-{session}-{segment}-{offset}"))
}

fn canonical_body(
    entry: &LoggedHistoryEntry,
    purpose: PublicMaterialUse,
) -> Option<CanonicalBody<'_>> {
    if let Some(input) = &entry.metadata.system_input {
        let usage = input.usage();
        let allowed = match purpose {
            PublicMaterialUse::Recording => usage.recording_material,
            PublicMaterialUse::MemoryEvidence => usage.memory_evidence,
        };
        if allowed == crate::InputEvidenceUse::Exclude {
            return None;
        }
        let public = input.public_item()?;
        if let LoggedItem::Message {
            role: LoggedRole::System,
            ..
        } = &entry.item
        {
            return Some(CanonicalBody::Message {
                origin: &entry.metadata.origin,
                role: LoggedRole::System,
                content: std::borrow::Cow::Owned(vec![crate::LoggedContentPart::Text {
                    text: public.body.clone(),
                }]),
            });
        }
        return None;
    }
    match &entry.item {
        LoggedItem::Message {
            role: LoggedRole::System,
            ..
        }
        | LoggedItem::Reasoning { .. }
        | LoggedItem::AssistantResponseBoundary { .. } => None,
        LoggedItem::Message { role, content } => Some(CanonicalBody::Message {
            origin: &entry.metadata.origin,
            role: *role,
            content: std::borrow::Cow::Borrowed(content),
        }),
        LoggedItem::ToolCall {
            call_id,
            name,
            arguments,
            ..
        } => Some(CanonicalBody::ToolCall {
            origin: &entry.metadata.origin,
            call_id,
            name,
            arguments,
        }),
        LoggedItem::ToolResult {
            call_id,
            summary,
            content,
            disposition,
            is_error,
            ..
        } => Some(CanonicalBody::ToolResult {
            origin: &entry.metadata.origin,
            call_id,
            summary,
            content,
            disposition,
            is_error: *is_error,
        }),
    }
}

fn canonical_metadata(
    entry: &LoggedHistoryEntry,
    purpose: PublicMaterialUse,
) -> Result<Option<(String, u64)>, PublicMaterialError> {
    let Some(body) = canonical_body(entry, purpose) else {
        return Ok(None);
    };
    let mut sink = BodySink::new(0, 0);
    serde_json::to_writer(&mut sink, &body)?;
    Ok(Some((hex_digest(&sink.digest.finalize()), sink.total)))
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

struct BodySink {
    digest: Sha256,
    total: u64,
    offset: u64,
    max: usize,
    bytes: Vec<u8>,
}

impl BodySink {
    fn new(offset: u64, max: usize) -> Self {
        Self {
            digest: Sha256::new(),
            total: 0,
            offset,
            max,
            bytes: Vec::new(),
        }
    }
    fn finish(mut self) -> Result<(String, u64, String), PublicMaterialError> {
        // The final byte may land inside a UTF-8 scalar. Never include a
        // partial scalar and never advance the caller over its missing bytes.
        loop {
            match std::str::from_utf8(&self.bytes) {
                Ok(_) => break,
                Err(e) if e.error_len().is_none() => {
                    self.bytes.truncate(e.valid_up_to());
                }
                Err(_) => return Err(PublicMaterialError::InvalidOffset),
            }
        }
        Ok((
            hex_digest(&self.digest.finalize()),
            self.total,
            String::from_utf8(self.bytes).map_err(|_| PublicMaterialError::InvalidOffset)?,
        ))
    }
}

impl Write for BodySink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.digest.update(bytes);
        let start = self
            .offset
            .saturating_sub(self.total)
            .min(bytes.len() as u64) as usize;
        let available = self.max.saturating_sub(self.bytes.len());
        let end = start.saturating_add(available).min(bytes.len());
        if end > start {
            self.bytes.extend_from_slice(&bytes[start..end]);
        }
        self.total = self
            .total
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| io::Error::other("canonical body length overflow"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct RecordReader {
    reader: BufReader<File>,
    offset: u64,
    end: u64,
    max: u64,
}

impl RecordReader {
    fn open(path: &Path, max: u64) -> Result<Self, PublicMaterialError> {
        // Do not follow symlinks in a caller-selected storage root or a log.
        let mut prefix = PathBuf::new();
        for component in path.components() {
            prefix.push(component);
            let metadata = std::fs::symlink_metadata(&prefix)?;
            if metadata.file_type().is_symlink() {
                return Err(PublicMaterialError::Unavailable);
            }
        }
        let file = open_noatime(path)?;
        if !file.metadata()?.is_file() {
            return Err(PublicMaterialError::Unavailable);
        }
        let end = file.metadata()?.len();
        Ok(Self {
            reader: BufReader::new(file),
            offset: 0,
            end,
            max,
        })
    }
    fn seek(&mut self, offset: u64) -> Result<(), PublicMaterialError> {
        self.reader.seek(SeekFrom::Start(offset))?;
        self.offset = offset;
        Ok(())
    }
    fn next(&mut self) -> Result<Option<(u64, LogEntry)>, PublicMaterialError> {
        if self.offset >= self.end {
            return Ok(None);
        }
        let mut bytes = Vec::new();
        let read = (&mut self.reader)
            .take(self.max.saturating_add(1).min(self.end - self.offset))
            .read_until(b'\n', &mut bytes)? as u64;
        if read > self.max {
            return Err(PublicMaterialError::ResourceLimit);
        }
        if bytes.last() != Some(&b'\n') {
            return Ok(None);
        } // Uncommitted tail.
        let offset = self.offset;
        self.offset += read;
        Ok(Some((offset, serde_json::from_slice(&bytes)?)))
    }
}

fn open_noatime(path: &Path) -> io::Result<File> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOATIME | libc::O_NOFOLLOW)
            .open(path)
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let _ = path;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "no-atime reads unavailable",
        ))
    }
}

#[cfg(test)]
mod tests;
