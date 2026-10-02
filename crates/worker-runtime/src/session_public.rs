use std::path::Path;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use runtime_api::{
    SessionPublicEntryKind, SessionPublicLineage, SessionPublicLineageKind,
    SessionPublicReadAvailability, SessionPublicReadMode, SessionPublicReadPage,
    SessionPublicReadRequest, SessionPublicSearchAvailability, SessionPublicSearchItem,
    SessionPublicSearchPage, SessionPublicSearchRequest, SessionPublicToolPart,
    SessionPublicUnavailableReason, WorkerSessionArchiveManifest,
};
use serde::{Deserialize, Serialize};
use session_store::{
    SessionPublicIndex, SessionPublicIndexEntry, SessionPublicIndexEntryKind,
    SessionPublicIndexLimits, SessionPublicIndexLineage, SessionPublicIndexOriginKind,
    SessionPublicIndexReadError, SessionPublicIndexScanPosition, SessionPublicIndexToolPart,
    read_session_public_index, read_session_public_index_page,
};

use crate::retention::WorkerSessionArchiveManifest as DomainArchiveManifest;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionPublicRuntimeCursor {
    generation: String,
    position: SessionPublicIndexScanPosition,
}

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
    let cursor = match request.scan_cursor.as_deref().map(decode_scan_cursor) {
        Some(Ok(cursor)) => Some(cursor),
        Some(Err(())) => {
            return unavailable(
                SessionPublicUnavailableReason::InvalidCursor,
                "Session public search cursor is invalid",
            );
        }
        None => None,
    };
    if let (Some(expected), Some(cursor)) =
        (request.expected_generation.as_deref(), cursor.as_ref())
        && expected != cursor.generation
    {
        return unavailable(
            SessionPublicUnavailableReason::InvalidCursor,
            "Session public search cursor generation changed",
        );
    }

    let query = request
        .query
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_lowercase);
    let expected_generation = request
        .expected_generation
        .as_deref()
        .or_else(|| cursor.as_ref().map(|cursor| cursor.generation.as_str()));
    let index = match read_session_public_index_page(
        session_root,
        SessionPublicIndexLimits {
            max_bytes: request.max_scan_bytes,
            max_segments: request.max_segments,
            max_entries: request.max_entries,
        },
        expected_generation,
        cursor.as_ref().map(|cursor| cursor.position.clone()),
        request.limit,
        |entry| matches_entry(entry, request, query.as_deref()),
    ) {
        Ok(index) => index,
        Err(error) => {
            let (reason, message) = map_index_error(error);
            return unavailable(reason, message);
        }
    };
    if index.session_id != request.expected_session_id {
        return unavailable(
            SessionPublicUnavailableReason::NotFound,
            "Session identity does not match the authorized source",
        );
    }

    let mut items = Vec::with_capacity(index.entries.len());
    for item in index.entries {
        let scan_cursor = match encode_scan_cursor(&SessionPublicRuntimeCursor {
            generation: index.generation.clone(),
            position: item.scan_from,
        }) {
            Ok(cursor) => cursor,
            Err(()) => {
                return unavailable(
                    SessionPublicUnavailableReason::StorageUnavailable,
                    "Session public search cursor could not be encoded",
                );
            }
        };
        let entry = item.entry;
        items.push(SessionPublicSearchItem {
            segment_id: item.segment_id,
            entry_ref: entry.entry_ref,
            kind: entry_kind(entry.kind),
            origin: entry.provenance,
            tool_name: entry.tool_name,
            tool_part: entry.tool_part.map(tool_part),
            compact: entry.compact_text,
            compact_truncated: entry.compact_text_truncated,
            lineage: lineage(&item.lineage),
            scan_cursor,
        });
    }
    let next_scan_cursor = match index.next_position {
        Some(position) => match encode_scan_cursor(&SessionPublicRuntimeCursor {
            generation: index.generation.clone(),
            position,
        }) {
            Ok(cursor) => Some(cursor),
            Err(()) => {
                return unavailable(
                    SessionPublicUnavailableReason::StorageUnavailable,
                    "Session public search cursor could not be encoded",
                );
            }
        },
        None => None,
    };
    SessionPublicSearchAvailability::Page {
        page: SessionPublicSearchPage {
            session_id: index.session_id,
            generation: index.generation,
            items,
            next_scan_cursor,
            has_more: index.has_more,
            scanned_bytes: index.scanned_bytes,
            scanned_segments: index.scanned_segments,
            scanned_entries: index.scanned_entries,
            archive_manifest: archive_manifest.map(api_archive_manifest),
        },
    }
}

fn encode_scan_cursor(cursor: &SessionPublicRuntimeCursor) -> Result<String, ()> {
    serde_json::to_vec(cursor)
        .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
        .map_err(|_| ())
}

fn decode_scan_cursor(cursor: &str) -> Result<SessionPublicRuntimeCursor, ()> {
    let bytes = URL_SAFE_NO_PAD.decode(cursor).map_err(|_| ())?;
    serde_json::from_slice(&bytes).map_err(|_| ())
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
        SessionPublicIndexReadError::StaleCursor => (
            SessionPublicUnavailableReason::InvalidCursor,
            "Session public search cursor is stale",
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

#[cfg(test)]
mod tests {
    use agen::llm_client::RequestConfig;
    use session_store::{
        LogEntry, LoggedContentPart, LoggedHistoryEntry, LoggedItem, LoggedRole,
        LoggedSessionHistoryEntryId, LoggedSessionHistoryMetadata, LoggedSessionHistoryOrigin,
        Store, WorkerSessionStore,
    };

    use super::*;

    fn message(id: &str, text: &str) -> LoggedHistoryEntry {
        LoggedHistoryEntry {
            item: LoggedItem::Message {
                role: LoggedRole::User,
                content: vec![LoggedContentPart::Text { text: text.into() }],
            },
            metadata: LoggedSessionHistoryMetadata {
                entry_id: LoggedSessionHistoryEntryId(id.to_string()),
                origin: LoggedSessionHistoryOrigin::HumanInput {
                    account_id: "account-1".to_string(),
                },
                derivation: None,
            },
        }
    }

    fn request(
        session_id: session_store::SessionId,
        generation: Option<String>,
        scan_cursor: Option<String>,
    ) -> SessionPublicSearchRequest {
        SessionPublicSearchRequest {
            workspace_id: "workspace-1".to_string(),
            source: runtime_api::SessionPublicSource::Retained {
                worker_id: crate::identity::WorkerId::now_v7(),
            },
            expected_session_id: session_id.to_string(),
            expected_generation: generation,
            query: Some("needle".to_string()),
            kind: None,
            tool_name: None,
            tool_part: SessionPublicToolPart::Both,
            scan_cursor,
            limit: 20,
            max_scan_bytes: 64 * 1024 * 1024,
            max_segments: 64,
            max_entries: 100_000,
        }
    }

    #[test]
    fn search_continues_beyond_segment_budget_and_stales_after_mutation() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("session");
        let store = WorkerSessionStore::new(&root).unwrap();
        let session_id = session_store::new_session_id();
        let mut last_segment = None;
        for index in 0..65_u128 {
            let segment_id = uuid::Uuid::from_u128(10_000 + index);
            last_segment = Some(segment_id);
            store
                .create_segment(
                    session_id,
                    segment_id,
                    &[LogEntry::AnnotatedSegmentStart {
                        ts: 1,
                        session_id,
                        system_prompt: None,
                        config: RequestConfig::default(),
                        history: vec![message(
                            &format!("entry-{index}"),
                            if index == 64 { "needle" } else { "ordinary" },
                        )],
                        forked_from: None,
                        compacted_from: None,
                    }],
                )
                .unwrap();
        }

        let first = match search(&root, &request(session_id, None, None), None) {
            SessionPublicSearchAvailability::Page { page } => page,
            other => panic!("unexpected first search result: {other:?}"),
        };
        assert!(first.items.is_empty());
        assert!(first.has_more);
        let cursor = first.next_scan_cursor.clone().unwrap();

        let second = match search(
            &root,
            &request(
                session_id,
                Some(first.generation.clone()),
                Some(cursor.clone()),
            ),
            None,
        ) {
            SessionPublicSearchAvailability::Page { page } => page,
            other => panic!("unexpected continued search result: {other:?}"),
        };
        assert_eq!(second.items.len(), 1);
        assert_eq!(second.items[0].compact, "needle");
        assert!(!second.has_more);

        store
            .append(
                session_id,
                last_segment.unwrap(),
                &LogEntry::AnnotatedAssistantItem {
                    ts: 2,
                    entry: message("mutation", "changed"),
                },
            )
            .unwrap();
        assert!(matches!(
            search(
                &root,
                &request(session_id, Some(first.generation), Some(cursor)),
                None,
            ),
            SessionPublicSearchAvailability::Unavailable {
                reason: SessionPublicUnavailableReason::InvalidCursor,
                ..
            }
        ));
    }
}
