use std::path::Path;

use runtime_api::{
    SessionPublicEntryKind, SessionPublicLineage, SessionPublicLineageKind,
    SessionPublicReadAvailability, SessionPublicReadMode, SessionPublicReadPage,
    SessionPublicReadRequest, SessionPublicSearchAvailability, SessionPublicSearchItem,
    SessionPublicSearchPage, SessionPublicSearchRequest, SessionPublicToolPart,
    SessionPublicUnavailableReason, WorkerSessionArchiveManifest,
};
use session_store::{
    SessionPublicIndex, SessionPublicIndexEntry, SessionPublicIndexEntryKind,
    SessionPublicIndexLimits, SessionPublicIndexLineage, SessionPublicIndexOriginKind,
    SessionPublicIndexReadError, SessionPublicIndexToolPart, read_session_public_index,
};

use crate::retention::WorkerSessionArchiveManifest as DomainArchiveManifest;

pub(crate) fn search(
    session_root: &Path,
    request: &SessionPublicSearchRequest,
    archive_manifest: Option<&DomainArchiveManifest>,
) -> SessionPublicSearchAvailability {
    if request.limit == 0 || request.limit > 100 {
        return unavailable(
            SessionPublicUnavailableReason::ResourceLimit,
            "Session public search limit must be between 1 and 100",
        );
    }
    let index = match load_index(
        session_root,
        request.max_scan_bytes,
        request.max_segments,
        request.max_entries,
    ) {
        Ok(index) => index,
        Err((reason, message)) => return unavailable(reason, message),
    };
    if index.session_id != request.expected_session_id {
        return unavailable(
            SessionPublicUnavailableReason::NotFound,
            "Session identity does not match the authorized source",
        );
    }
    if request
        .expected_generation
        .as_deref()
        .is_some_and(|expected| expected != index.generation)
    {
        return unavailable(
            SessionPublicUnavailableReason::InvalidCursor,
            "Session public search generation changed",
        );
    }

    let query = request
        .query
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_lowercase);
    let mut matches = Vec::new();
    for segment in &index.segments {
        for entry in &segment.entries {
            if !matches_entry(entry, request, query.as_deref()) {
                continue;
            }
            matches.push(SessionPublicSearchItem {
                segment_id: segment.segment_id.clone(),
                entry_ref: entry.entry_ref.clone(),
                kind: entry_kind(entry.kind),
                origin: entry.provenance.clone(),
                tool_name: entry.tool_name.clone(),
                tool_part: entry.tool_part.map(tool_part),
                compact: entry.compact_text.clone(),
                lineage: lineage(&segment.lineage),
            });
        }
    }
    if request.offset > matches.len() {
        return unavailable(
            SessionPublicUnavailableReason::InvalidCursor,
            "Session public search position is stale",
        );
    }
    let end = request
        .offset
        .saturating_add(request.limit)
        .min(matches.len());
    let items = matches[request.offset..end].to_vec();
    let has_more = end < matches.len();
    SessionPublicSearchAvailability::Page {
        page: SessionPublicSearchPage {
            session_id: index.session_id,
            generation: index.generation,
            items,
            next_offset: has_more.then_some(end),
            has_more,
            scanned_bytes: index.scanned_bytes,
            scanned_segments: index.segments.len(),
            scanned_entries: index.scanned_entries,
            archive_manifest: archive_manifest.map(api_archive_manifest),
        },
    }
}

pub(crate) fn read(
    session_root: &Path,
    request: &SessionPublicReadRequest,
    archive_manifest: Option<&DomainArchiveManifest>,
) -> SessionPublicReadAvailability {
    if request.max_content_bytes == 0 {
        return read_unavailable(
            SessionPublicUnavailableReason::ResourceLimit,
            "Session public read content limit must be positive",
        );
    }
    let index = match load_index(
        session_root,
        request.max_scan_bytes,
        request.max_segments,
        request.max_entries,
    ) {
        Ok(index) => index,
        Err((reason, message)) => return read_unavailable(reason, message),
    };
    if index.session_id != request.expected_session_id {
        return read_unavailable(
            SessionPublicUnavailableReason::NotFound,
            "Session identity does not match the authorized source",
        );
    }
    if request
        .expected_generation
        .as_deref()
        .is_some_and(|expected| expected != index.generation)
    {
        return read_unavailable(
            SessionPublicUnavailableReason::InvalidCursor,
            "Session public read generation changed",
        );
    }
    let Some(segment) = index
        .segments
        .iter()
        .find(|segment| segment.segment_id == request.segment_id)
    else {
        return read_unavailable(
            SessionPublicUnavailableReason::NotFound,
            "Session entry was not found in the authorized source",
        );
    };
    let mut entries = segment
        .entries
        .iter()
        .filter(|entry| entry.entry_ref == request.entry_ref);
    let Some(entry) = entries.next() else {
        return read_unavailable(
            SessionPublicUnavailableReason::NotFound,
            "Session entry was not found in the authorized source",
        );
    };
    if entries.next().is_some() {
        return read_unavailable(
            SessionPublicUnavailableReason::CorruptLog,
            "Session entry identity is ambiguous in its Segment",
        );
    }
    let content = match request.mode {
        SessionPublicReadMode::Compact => &entry.compact_text,
        SessionPublicReadMode::Full => &entry.full_text,
    };
    if request.byte_offset > content.len() || !content.is_char_boundary(request.byte_offset) {
        return read_unavailable(
            SessionPublicUnavailableReason::InvalidCursor,
            "Session public read position is stale",
        );
    }
    let mut end = request
        .byte_offset
        .saturating_add(request.max_content_bytes)
        .min(content.len());
    while end > request.byte_offset && !content.is_char_boundary(end) {
        end -= 1;
    }
    if end == request.byte_offset && request.byte_offset < content.len() {
        return read_unavailable(
            SessionPublicUnavailableReason::ResourceLimit,
            "Session public read cannot fit one UTF-8 scalar in the content limit",
        );
    }
    let has_more = end < content.len();
    SessionPublicReadAvailability::Page {
        page: SessionPublicReadPage {
            session_id: index.session_id,
            generation: index.generation,
            segment_id: segment.segment_id.clone(),
            entry_ref: entry.entry_ref.clone(),
            kind: entry_kind(entry.kind),
            origin: entry.provenance.clone(),
            lineage: lineage(&segment.lineage),
            mode: request.mode,
            content: content[request.byte_offset..end].to_string(),
            next_byte_offset: has_more.then_some(end),
            has_more,
            scanned_bytes: index.scanned_bytes,
            scanned_segments: index.segments.len(),
            scanned_entries: index.scanned_entries,
            archive_manifest: archive_manifest.map(api_archive_manifest),
        },
    }
}

fn load_index(
    session_root: &Path,
    max_bytes: u64,
    max_segments: usize,
    max_entries: usize,
) -> Result<SessionPublicIndex, (SessionPublicUnavailableReason, &'static str)> {
    read_session_public_index(
        session_root,
        SessionPublicIndexLimits {
            max_bytes,
            max_segments,
            max_entries,
        },
    )
    .map_err(map_index_error)
}

fn matches_entry(
    entry: &SessionPublicIndexEntry,
    request: &SessionPublicSearchRequest,
    query: Option<&str>,
) -> bool {
    if request
        .kind
        .is_some_and(|kind| kind != entry_kind(entry.kind))
    {
        return false;
    }
    if request.tool_name.is_some() || request.tool_part != SessionPublicToolPart::Both {
        if entry.kind != SessionPublicIndexEntryKind::Tool {
            return false;
        }
    }
    if request
        .tool_name
        .as_deref()
        .is_some_and(|name| entry.tool_name.as_deref() != Some(name))
    {
        return false;
    }
    if request.tool_part != SessionPublicToolPart::Both
        && entry.tool_part.map(tool_part) != Some(request.tool_part)
    {
        return false;
    }
    query.is_none_or(|query| entry.full_text.to_lowercase().contains(query))
}

fn entry_kind(kind: SessionPublicIndexEntryKind) -> SessionPublicEntryKind {
    match kind {
        SessionPublicIndexEntryKind::User => SessionPublicEntryKind::User,
        SessionPublicIndexEntryKind::Assistant => SessionPublicEntryKind::Assistant,
        SessionPublicIndexEntryKind::Tool => SessionPublicEntryKind::Tool,
    }
}

fn tool_part(part: SessionPublicIndexToolPart) -> SessionPublicToolPart {
    match part {
        SessionPublicIndexToolPart::Input => SessionPublicToolPart::Input,
        SessionPublicIndexToolPart::Output => SessionPublicToolPart::Output,
    }
}

fn lineage(value: &SessionPublicIndexLineage) -> SessionPublicLineage {
    SessionPublicLineage {
        kind: match value.origin_kind {
            SessionPublicIndexOriginKind::Root => SessionPublicLineageKind::Root,
            SessionPublicIndexOriginKind::Fork => SessionPublicLineageKind::Fork,
            SessionPublicIndexOriginKind::Compact => SessionPublicLineageKind::Compact,
        },
        parent_segment_id: value.parent_segment_id.clone(),
        at_turn_index: value.parent_turn_index,
    }
}

fn map_index_error(
    error: SessionPublicIndexReadError,
) -> (SessionPublicUnavailableReason, &'static str) {
    match error {
        SessionPublicIndexReadError::MigrationRequired => (
            SessionPublicUnavailableReason::MigrationRequired,
            "Session storage requires migration",
        ),
        SessionPublicIndexReadError::Missing => (
            SessionPublicUnavailableReason::RetentionMissing,
            "Session storage is missing",
        ),
        SessionPublicIndexReadError::Corrupt => (
            SessionPublicUnavailableReason::CorruptLog,
            "Session log is corrupt",
        ),
        SessionPublicIndexReadError::Storage => (
            SessionPublicUnavailableReason::StorageUnavailable,
            "Session storage is unavailable",
        ),
        SessionPublicIndexReadError::ResourceLimit => (
            SessionPublicUnavailableReason::ResourceLimit,
            "Session scan exceeds a resource limit",
        ),
    }
}

fn unavailable(
    reason: SessionPublicUnavailableReason,
    message: impl Into<String>,
) -> SessionPublicSearchAvailability {
    SessionPublicSearchAvailability::Unavailable {
        reason,
        message: message.into(),
    }
}

fn read_unavailable(
    reason: SessionPublicUnavailableReason,
    message: impl Into<String>,
) -> SessionPublicReadAvailability {
    SessionPublicReadAvailability::Unavailable {
        reason,
        message: message.into(),
    }
}

fn api_archive_manifest(manifest: &DomainArchiveManifest) -> WorkerSessionArchiveManifest {
    WorkerSessionArchiveManifest {
        schema_version: manifest.schema_version,
        archive_id: manifest.archive_id.clone(),
        workspace_id: manifest.workspace_id.clone(),
        source_runtime_id: manifest.source_runtime_id.clone(),
        source_worker_id: manifest.source_worker_id,
        source_session_id: manifest.source_session_id.clone(),
        segment_ids: manifest.segment_ids.clone(),
        source_created_at: manifest.source_created_at.clone(),
        removed_at: manifest.removed_at.clone(),
        archived_at_unix_seconds: manifest.archived_at_unix_seconds,
        effective_profile: manifest.effective_profile.clone(),
        retention_class: manifest.retention_class.clone(),
        content_checksum_sha256: manifest.content_checksum_sha256.clone(),
        content_bytes: manifest.content_bytes,
        content_file_count: manifest.content_file_count,
        policy_id: manifest.policy_id.clone(),
        policy_revision: manifest.policy_revision,
        operation_id: manifest.operation_id.clone(),
        input_fingerprint: manifest.input_fingerprint.clone(),
    }
}
