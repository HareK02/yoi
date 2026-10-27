//! Host capture and pure canonical slicing. No Job state or live-reader fallback lives here.

use super::*;
use crate::committed_replay::CommittedPublicReplay;
use crate::public_material::{
    PublicMaterialDescriptor, PublicMaterialError, PublicMaterialLimits, PublicMaterialUse,
};
use crate::retention::SessionRetentionStore;
use crate::{SegmentId, SessionId};
use std::path::Path;

/// Continuation in canonical Journal order, not an observation/Memory view cursor.
/// A Host must reconcile the descriptor at this position with its saved checkpoint.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingPosition {
    pub canonical_order: u64,
    pub byte_offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingCaptureSlice {
    pub capture: CaptureDto,
    pub next: Option<RecordingPosition>,
}

/// Inspect recording admission descriptors without constructing a body-bearing DTO.
///
/// The Host must authorize the source and hold the **same** Session publication/erasure
/// guard from before reading its persisted active pointer until this function returns
/// (including the callback). Pass that pointer's Segment, not a filename/mtime-derived
/// selector. This function verifies the guard and stored Session identities but does
/// not authorize the caller or establish that the supplied guard fences this root.
/// No lock is recursively acquired, no active pointer is reread, and no replay handle
/// or body-read capability is exposed to `inspect`.
///
/// The common committed recording replay fixes per-file ends before decoding. It
/// retains only descriptors/locations, decoding one record at a time and streaming
/// canonical bodies into their digests. `replay_limits` bound record allocation,
/// aggregate storage extent, entries and Segments; there is no full-capture text/JSON
/// allocation or `CaptureLimits` ceiling. The callback receives only descriptors in
/// canonical order and may return a Host-specific mapping (including a `Result`). It
/// runs once on success and not at all on replay/identity/resource errors.
///
/// This catalog is for Host admission, **not** a descriptor-only Job input. Construct
/// the Job's bounded body input with the guarded slice API at its own authorized
/// boundary; after saving it, execute only that immutable value. Neither descriptors
/// nor a completed callback authorize delayed adoption after erasure.
///
/// ```no_run
/// # use std::path::Path;
/// # use session_store::{SessionId, SegmentId};
/// # use session_store::retention::SessionRetentionGuard;
/// # use session_store::public_material::PublicMaterialError;
/// # fn admit(root: &Path, session: SessionId, active: SegmentId,
/// #          guard: &SessionRetentionGuard) -> Result<Vec<u64>, PublicMaterialError> {
/// // `active` was resolved from persisted Host metadata while holding `guard`.
/// session_store::capture::recording_catalog_worker_guarded(
///     root, session, active, Default::default(), guard,
///     |descriptors| descriptors.iter().map(|d| d.total_bytes).collect(),
/// )
/// # }
/// ```
pub fn recording_catalog_worker_guarded<T>(
    session_root: &Path,
    expected_session: SessionId,
    active_segment: SegmentId,
    replay_limits: PublicMaterialLimits,
    guard: &crate::retention::SessionRetentionGuard,
    inspect: impl FnOnce(&[PublicMaterialDescriptor]) -> T,
) -> Result<T, PublicMaterialError> {
    if guard.session_id() != expected_session {
        return Err(PublicMaterialError::Inconsistent);
    }
    let replay = CommittedPublicReplay::open_worker_guarded(
        session_root,
        active_segment,
        replay_limits,
        None,
        PublicMaterialUse::Recording,
    )?;
    if replay.session_id() != expected_session {
        return Err(PublicMaterialError::Inconsistent);
    }
    Ok(inspect(&replay.into_recording_descriptors()))
}

/// FsStore-layout counterpart of [`recording_catalog_worker_guarded`].
///
/// `sessions_root` is the directory containing per-Session directories. Invoke this
/// inside the Host's `with_recording_boundary` callback, passing **its existing guard
/// and persisted active Segment**. That same external retention control store must
/// fence publishers, pointer changes, archive moves and erasure. Do not reacquire the
/// guard or use a separately opened legacy public reader inside the boundary.
/// All descriptor-only, allocation and callback guarantees of the Worker variant
/// apply; this function does not construct an unbounded transient `CaptureDto`.
pub fn recording_catalog_fs_guarded<T>(
    sessions_root: &Path,
    session: SessionId,
    active_segment: SegmentId,
    replay_limits: PublicMaterialLimits,
    guard: &crate::retention::SessionRetentionGuard,
    inspect: impl FnOnce(&[PublicMaterialDescriptor]) -> T,
) -> Result<T, PublicMaterialError> {
    if guard.session_id() != session {
        return Err(PublicMaterialError::Inconsistent);
    }
    let replay = CommittedPublicReplay::open_fs_guarded(
        sessions_root,
        session,
        active_segment,
        replay_limits,
    )?;
    Ok(inspect(&replay.into_recording_descriptors()))
}

/// Capture the selected original lineage, including typed Host run boundaries.
/// The caller must hold the Session publication/erasure guard while selecting the
/// active pointer and through this call. Runtime publishers use this same lock
/// for appends and Segment creation. No lock is recursively acquired here.
pub fn capture_recording_worker_guarded(
    session_root: &Path,
    expected_session: SessionId,
    active_segment: SegmentId,
    replay_limits: PublicMaterialLimits,
    capture_limits: CaptureLimits,
    guard: &crate::retention::SessionRetentionGuard,
) -> Result<CaptureDto, PublicMaterialError> {
    if guard.session_id() != expected_session {
        return Err(PublicMaterialError::Inconsistent);
    }
    let replay = CommittedPublicReplay::open_worker_guarded(
        session_root,
        active_segment,
        replay_limits,
        None,
        PublicMaterialUse::Recording,
    )?;
    if replay.session_id() != expected_session {
        return Err(PublicMaterialError::Inconsistent);
    }
    replay.into_recording_dto(capture_limits)
}

/// Standalone convenience entry; Hosts resolving a mutable active pointer should
/// instead hold the fence while resolving it and call the guarded entry.
pub fn capture_recording_worker(
    session_root: &Path,
    expected_session: SessionId,
    active_segment: SegmentId,
    retention: &SessionRetentionStore,
    replay_limits: PublicMaterialLimits,
    capture_limits: CaptureLimits,
) -> Result<CaptureDto, PublicMaterialError> {
    let guard = retention
        .lock_live(expected_session)
        .map_err(|error| match error {
            crate::StoreError::SessionErased(_) => PublicMaterialError::Erased,
            _ => PublicMaterialError::Unavailable,
        })?;
    capture_recording_worker_guarded(
        session_root,
        expected_session,
        active_segment,
        replay_limits,
        capture_limits,
        &guard,
    )
}

/// Bounded construction for one Job input: descriptor replay is bounded separately;
/// only at most the requested body budget is materialized from the fixed files.
pub fn capture_recording_slice_worker_guarded(
    session_root: &Path,
    expected_session: SessionId,
    active_segment: SegmentId,
    replay_limits: PublicMaterialLimits,
    start: RecordingPosition,
    max_entries: usize,
    max_serialized_bytes: usize,
    guard: &crate::retention::SessionRetentionGuard,
) -> Result<RecordingCaptureSlice, PublicMaterialError> {
    if guard.session_id() != expected_session {
        return Err(PublicMaterialError::Inconsistent);
    }
    let replay = CommittedPublicReplay::open_worker_guarded(
        session_root,
        active_segment,
        replay_limits,
        None,
        PublicMaterialUse::Recording,
    )?;
    if replay.session_id() != expected_session {
        return Err(PublicMaterialError::Inconsistent);
    }
    replay.recording_slice(start, max_entries, max_serialized_bytes)
}

/// The standalone FsStore layout uses the same committed replay and fixed values.
pub fn capture_recording_fs_guarded(
    sessions_root: &Path,
    session: SessionId,
    active_segment: SegmentId,
    replay_limits: PublicMaterialLimits,
    capture_limits: CaptureLimits,
    guard: &crate::retention::SessionRetentionGuard,
) -> Result<CaptureDto, PublicMaterialError> {
    if guard.session_id() != session {
        return Err(PublicMaterialError::Inconsistent);
    }
    CommittedPublicReplay::open_fs_guarded(sessions_root, session, active_segment, replay_limits)?
        .into_recording_dto(capture_limits)
}

/// Bounded standalone capture; the caller holds its FsStore retention fence.
pub fn capture_recording_slice_fs_guarded(
    sessions_root: &Path,
    session: SessionId,
    active_segment: SegmentId,
    replay_limits: PublicMaterialLimits,
    start: RecordingPosition,
    max_entries: usize,
    max_serialized_bytes: usize,
    guard: &crate::retention::SessionRetentionGuard,
) -> Result<RecordingCaptureSlice, PublicMaterialError> {
    if guard.session_id() != session {
        return Err(PublicMaterialError::Inconsistent);
    }
    CommittedPublicReplay::open_fs_guarded(sessions_root, session, active_segment, replay_limits)?
        .recording_slice(start, max_entries, max_serialized_bytes)
}

impl CaptureDto {
    /// Select the oldest canonical prefix fitting both budgets, including JSON
    /// escaping, metadata and continuation. Only immutable owned bytes are read.
    /// A later Job can recapture at `next`; never combine live pages into this value.
    pub fn recording_slice(
        &self,
        start: RecordingPosition,
        max_entries: usize,
        max_serialized_bytes: usize,
    ) -> Result<RecordingCaptureSlice, CaptureError> {
        if max_entries == 0 {
            return Err(CaptureError::BudgetTooSmall);
        }
        if start.canonical_order > self.view_entry_count {
            return Err(CaptureError::InvalidRange);
        }
        let mut result = RecordingCaptureSlice {
            capture: CaptureDto {
                version: self.version,
                session_id: self.session_id.clone(),
                segment_id: self.segment_id.clone(),
                view_entry_count: self.view_entry_count,
                entries: Vec::new(),
            },
            next: (start.canonical_order < self.view_entry_count).then_some(start),
        };
        if body::json_size(&result, max_serialized_bytes).is_err() {
            return Err(CaptureError::BudgetTooSmall);
        }
        if start.canonical_order == self.view_entry_count {
            if start.byte_offset != 0 {
                return Err(CaptureError::InvalidRange);
            }
            result.capture.validate(CaptureLimits {
                max_bytes: max_serialized_bytes,
                max_text_bytes: max_serialized_bytes,
                max_entries,
                ..Default::default()
            })?;
            return Ok(result);
        }
        let first = self
            .entries
            .iter()
            .position(|entry| entry.order == start.canonical_order)
            .ok_or(CaptureError::InvalidRange)?;
        let mut expected_order = start.canonical_order;
        for entry in self.entries.iter().skip(first).take(max_entries) {
            if entry.order != expected_order {
                return Err(CaptureError::InvalidRange);
            }
            let CaptureBody::CanonicalPublic {
                descriptor,
                byte_offset,
                text,
                boundary,
            } = &entry.body
            else {
                return Err(CaptureError::Invalid("not a recording capture"));
            };
            if !entry.policy.recording_material {
                return Err(CaptureError::PolicyDenied);
            }
            let offset = if entry.order == start.canonical_order {
                start.byte_offset
            } else {
                *byte_offset
            };
            let local = offset
                .checked_sub(*byte_offset)
                .and_then(|o| usize::try_from(o).ok())
                .ok_or(CaptureError::InvalidRange)?;
            if local > text.len()
                || !text.is_char_boundary(local)
                || offset >= descriptor.total_bytes
            {
                return Err(CaptureError::InvalidRange);
            }
            let item = CaptureEntryDto {
                entry_id: entry.entry_id.clone(),
                order: entry.order,
                source: entry.source.clone(),
                policy: entry.policy,
                body: CaptureBody::CanonicalPublic {
                    descriptor: descriptor.clone(),
                    byte_offset: offset,
                    text: String::new(),
                    boundary: *boundary,
                },
            };
            result.capture.entries.push(item);
            // At most budget bytes are considered before allocating a trial fragment.
            let mut low = 0;
            let mut high = text.len().saturating_sub(local).min(max_serialized_bytes);
            let prior_next = result.next;
            while low < high {
                let mid = low + (high - low).div_ceil(2);
                let mut end = local + mid;
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                if let CaptureBody::CanonicalPublic { text: fragment, .. } =
                    &mut result.capture.entries.last_mut().unwrap().body
                {
                    *fragment = text[local..end].to_owned();
                }
                let next_offset = offset + (end - local) as u64;
                result.next = if next_offset < descriptor.total_bytes {
                    Some(RecordingPosition {
                        canonical_order: entry.order,
                        byte_offset: next_offset,
                    })
                } else {
                    (entry.order + 1 < self.view_entry_count).then_some(RecordingPosition {
                        canonical_order: entry.order + 1,
                        byte_offset: 0,
                    })
                };
                if body::json_size(&result, max_serialized_bytes).is_ok() {
                    low = mid;
                } else {
                    high = mid - 1;
                }
            }
            let mut end = local + low;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            if end == local {
                result.capture.entries.pop();
                result.next = prior_next;
                if result.capture.entries.is_empty() {
                    return Err(CaptureError::BudgetTooSmall);
                }
                break;
            }
            if let CaptureBody::CanonicalPublic { text: fragment, .. } =
                &mut result.capture.entries.last_mut().unwrap().body
            {
                *fragment = text[local..end].to_owned();
            }
            let next_offset = offset + (end - local) as u64;
            result.next = if next_offset < descriptor.total_bytes {
                Some(RecordingPosition {
                    canonical_order: entry.order,
                    byte_offset: next_offset,
                })
            } else {
                (entry.order + 1 < self.view_entry_count).then_some(RecordingPosition {
                    canonical_order: entry.order + 1,
                    byte_offset: 0,
                })
            };
            if next_offset < descriptor.total_bytes {
                break;
            }
            expected_order = entry.order + 1;
        }
        body::json_size(&result, max_serialized_bytes).map_err(|_| CaptureError::BudgetTooSmall)?;
        result.capture.validate(CaptureLimits {
            max_bytes: max_serialized_bytes,
            max_text_bytes: max_serialized_bytes,
            max_entries,
            ..Default::default()
        })?;
        Ok(result)
    }

    /// Resolve canonical text already in the value. No original storage is consulted.
    pub fn recording_text(
        &self,
        order: u64,
    ) -> Result<(&crate::public_material::PublicMaterialDescriptor, u64, &str), CaptureError> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.order == order)
            .ok_or(CaptureError::UnknownReference)?;
        if !entry.policy.recording_material {
            return Err(CaptureError::PolicyDenied);
        }
        match &entry.body {
            CaptureBody::CanonicalPublic {
                descriptor,
                byte_offset,
                text,
                ..
            } => Ok((descriptor, *byte_offset, text)),
            _ => Err(CaptureError::Invalid("not canonical recording text")),
        }
    }
}
