//! Subject-scoped observation of standalone Sessions. This module owns no
//! Session body or projection: it uses the shared strict committed public index
//! reader for the FsStore layout, never a raw snapshot projector.

use std::path::{Path, PathBuf};

use protocol::WorkerId;
use serde::{Deserialize, Serialize};
use server_api::*;
use session_store::{
    SessionId, SessionPublicIndex, SessionPublicIndexEntry, SessionPublicIndexEntryKind,
    SessionPublicIndexLimits, SessionPublicIndexLineage, SessionPublicIndexOriginKind,
    SessionPublicIndexReadError, SessionPublicIndexToolPart,
};
use subjektiv::{SubjectSessionAttribution, SubjektivStore};
use worker::subjektiv::SubjektivHostError;

use crate::store::{StandaloneStoreError, StandaloneWorkerStore};

/// Adapter to the *shared* strict committed FsStore public projection. It must
/// be observational, enforce all supplied limits, and ignore unterminated tails,
/// system records, reasoning, and output without a persisted commit boundary.
pub(crate) type PublicIndexReader = fn(
    &Path,
    SessionId,
    SessionPublicIndexLimits,
) -> Result<SessionPublicIndex, SessionPublicIndexReadError>;

const MAX_SCANNED_SESSIONS: usize = 100;
const MAX_SCAN_BYTES: u64 = 8 * 1024 * 1024;
const MAX_SCAN_ENTRIES: usize = 20_000;
const MAX_SCAN_SEGMENTS: usize = 64;
const ITEM_BUDGET: usize = 28 * 1024;
const MAX_CURSOR_BYTES: usize = 24 * 1024;
const MAX_LINES: usize = 200;
const CURSOR_PREFIX: &str = "standalone-session-v1:";
const END_OF_TIME: &str = "9999-12-31T23:59:59.999999999Z";

type Failure = SubjektivSessionErrorResponse;
type LocalResult<T> = Result<T, Failure>;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    scope_id: String,
    subject_id: String,
    operation: String,
    filters: serde_json::Value,
    snapshot_at: String,
    after: Option<(String, String)>,
    /// Search resumes this Session before advancing the attribution keyset.
    current: Option<Position>,
    byte_offset: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Position {
    session_id: String,
    worker_id: String,
    generation: String,
    entry_offset: usize,
}

struct Backend<'a> {
    state_root: &'a Path,
    scope_id: &'a str,
    subject_id: &'a str,
    store: &'a SubjektivStore,
    reader: PublicIndexReader,
}

struct Authorized {
    attribution: SubjectSessionAttribution,
    sessions_path: PathBuf,
}

/// All scope arguments are the parent's trusted local binding, not values taken
/// from tool input. The projection is exclusively the shared FsStore reader.
pub(crate) fn execute(
    state_root: &Path,
    scope_id: &str,
    subject_id: &str,
    store: &SubjektivStore,
    operation: SubjektivSessionBackendOperation,
) -> Result<SubjektivSessionBackendResponse, SubjektivHostError> {
    execute_with_reader(
        state_root,
        scope_id,
        subject_id,
        store,
        operation,
        session_store::public_index::read_fs_session_public_index,
    )
}

fn execute_with_reader(
    state_root: &Path,
    scope_id: &str,
    subject_id: &str,
    store: &SubjektivStore,
    operation: SubjektivSessionBackendOperation,
    read_index: PublicIndexReader,
) -> Result<SubjektivSessionBackendResponse, SubjektivHostError> {
    let backend = Backend {
        state_root,
        scope_id,
        subject_id,
        store,
        reader: read_index,
    };
    let result = backend.check_scope().and_then(|()| match operation {
        SubjektivSessionBackendOperation::List(input) => backend.list(input),
        SubjektivSessionBackendOperation::Search(input) => backend.search(input),
        SubjektivSessionBackendOperation::Read(input) => backend.read(input),
    });
    Ok(match result {
        Ok(result) => {
            let response = SubjektivSessionBackendResponse::Ok { result };
            if fits(&response) {
                response
            } else {
                error_response(failure(
                    SubjektivSessionDiagnosticCode::ResourceLimit,
                    "Session response exceeds the serialized output budget",
                ))
            }
        }
        Err(error) => error_response(error),
    })
}

/// Evidence validation uses exactly the same attribution and Worker checks as
/// model reads. The parent can validate entry refs/ranges against this index;
/// no staging candidate is needed for a Session to be discoverable.
pub(crate) fn authorized_index(
    state_root: &Path,
    scope_id: &str,
    subject_id: &str,
    store: &SubjektivStore,
    session_id: &str,
) -> Result<SessionPublicIndex, SubjektivHostError> {
    authorized_index_with_reader(
        state_root,
        scope_id,
        subject_id,
        store,
        session_id,
        session_store::public_index::read_fs_session_public_index,
    )
}

fn authorized_index_with_reader(
    state_root: &Path,
    scope_id: &str,
    subject_id: &str,
    store: &SubjektivStore,
    session_id: &str,
    read_index: PublicIndexReader,
) -> Result<SessionPublicIndex, SubjektivHostError> {
    let backend = Backend {
        state_root,
        scope_id,
        subject_id,
        store,
        reader: read_index,
    };
    (|| {
        backend.check_scope()?;
        required_selector(session_id)?;
        let attribution = backend.attribution(session_id)?;
        let source = backend.authorize(attribution)?;
        backend.index(&source, limits())
    })()
    .map_err(|error: Failure| SubjektivHostError::Conflict {
        code: serde_json::to_value(error.code)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| "session_unavailable".into()),
        message: error.message,
    })
}

impl Backend<'_> {
    fn check_scope(&self) -> LocalResult<()> {
        if self.store.scope_id() != self.scope_id {
            return Err(denied());
        }
        if self
            .store
            .subject(self.subject_id)
            .map_err(|_| subject_unavailable())?
            .is_none()
        {
            return Err(subject_unavailable());
        }
        Ok(())
    }

    fn attribution(&self, session_id: &str) -> LocalResult<SubjectSessionAttribution> {
        self.store
            .session_attribution(session_id)
            .map_err(|_| subject_unavailable())?
            .filter(|a| a.subject_id == self.subject_id && a.runtime_id == self.scope_id)
            .ok_or_else(denied)
    }

    fn authorize(&self, attribution: SubjectSessionAttribution) -> LocalResult<Authorized> {
        if attribution.subject_id != self.subject_id || attribution.runtime_id != self.scope_id {
            return Err(denied());
        }
        let id: WorkerId = attribution.worker_id.parse().map_err(|_| denied())?;
        if !self.state_root.is_dir() {
            return Err(failure(
                SubjektivSessionDiagnosticCode::RetentionMissing,
                "Attributed standalone Worker state is missing",
            ));
        }
        let workers = StandaloneWorkerStore::open_existing(self.state_root).map_err(|_| {
            failure(
                SubjektivSessionDiagnosticCode::RuntimeUnavailable,
                "Standalone Worker storage is inaccessible",
            )
        })?;
        let record = workers.load(id).map_err(|error| match error {
            StandaloneStoreError::WorkerNotFound(_) => failure(
                SubjektivSessionDiagnosticCode::RetentionMissing,
                "Attributed standalone Worker record is missing",
            ),
            _ => failure(
                SubjektivSessionDiagnosticCode::CorruptLog,
                "Attributed standalone Worker record is unavailable or incomplete",
            ),
        })?;
        if record.worker_id != id
            || record.worker_id.to_string() != attribution.worker_id
            || !record.subject.as_ref().is_some_and(|binding| {
                binding.scope_id == self.scope_id && binding.subject_id == self.subject_id
            })
        {
            return Err(denied());
        }
        Ok(Authorized {
            attribution,
            sessions_path: workers.sessions_dir(id),
        })
    }

    fn index(
        &self,
        source: &Authorized,
        limits: SessionPublicIndexLimits,
    ) -> LocalResult<SessionPublicIndex> {
        let id: SessionId = source.attribution.session_id.parse().map_err(|_| {
            failure(
                SubjektivSessionDiagnosticCode::CorruptLog,
                "Attributed Session identity is invalid",
            )
        })?;
        let index = (self.reader)(&source.sessions_path, id, limits).map_err(index_failure)?;
        if index.session_id != source.attribution.session_id {
            return Err(failure(
                SubjektivSessionDiagnosticCode::CorruptLog,
                "Committed public index does not match the attributed Session",
            ));
        }
        if index.scanned_bytes > limits.max_bytes
            || index.scanned_entries > limits.max_entries
            || index.entry_count > limits.max_entries
            || index.segments.len() > limits.max_segments
        {
            return Err(failure(
                SubjektivSessionDiagnosticCode::ResourceLimit,
                "Committed public index exceeds the scan budget",
            ));
        }
        Ok(index)
    }

    fn cursor(
        &self,
        operation: &str,
        filters: serde_json::Value,
        token: Option<&str>,
    ) -> LocalResult<Cursor> {
        if let Some(token) = token {
            let cursor = decode_cursor(token)?;
            if cursor.scope_id != self.scope_id
                || cursor.subject_id != self.subject_id
                || cursor.operation != operation
                || cursor.filters != filters
                || cursor.snapshot_at.is_empty()
                || cursor.snapshot_at.len() > 128
            {
                return Err(stale());
            }
            Ok(cursor)
        } else {
            Ok(Cursor {
                scope_id: self.scope_id.into(),
                subject_id: self.subject_id.into(),
                operation: operation.into(),
                filters,
                snapshot_at: END_OF_TIME.into(),
                after: None,
                current: None,
                byte_offset: 0,
            })
        }
    }

    fn page(
        &self,
        cursor: &mut Cursor,
        selector: Option<&str>,
    ) -> LocalResult<subjektiv::SubjectSessionAttributionPage> {
        let page = self
            .store
            .subject_session_attribution_page(
                self.subject_id,
                selector,
                &cursor.snapshot_at,
                cursor
                    .after
                    .as_ref()
                    .map(|(at, id)| (at.as_str(), id.as_str())),
                1,
            )
            .map_err(|_| subject_unavailable())?;
        // Freeze discovery at the newest attribution on the first page, without
        // needing a second clock or reading the complete Subject history.
        if cursor.snapshot_at == END_OF_TIME {
            if let Some(first) = page.items.first() {
                cursor.snapshot_at = first.attributed_at.clone();
            }
        }
        Ok(page)
    }

    fn list(
        &self,
        input: SubjektivSessionListRequest,
    ) -> LocalResult<SubjektivSessionBackendResult> {
        let limit = limit(input.limit)?;
        let selector = optional_selector(input.session_id.as_deref())?;
        if let Some(id) = selector {
            self.authorize(self.attribution(id)?)?;
        }
        let mut cursor = self.cursor(
            "list",
            serde_json::json!({"session_id": selector, "storage": input.storage}),
            input.cursor.as_deref(),
        )?;
        if cursor.current.is_some() || cursor.byte_offset != 0 {
            return Err(stale());
        }
        let mut items = Vec::new();
        let mut has_more = false;
        for _ in 0..MAX_SCANNED_SESSIONS {
            let page = self.page(&mut cursor, selector)?;
            has_more = page.has_more && selector.is_none();
            let Some(attribution) = page.items.into_iter().next() else {
                break;
            };
            let after = (
                attribution.attributed_at.clone(),
                attribution.session_id.clone(),
            );
            if attribution.runtime_id != self.scope_id {
                cursor.after = Some(after);
                if !has_more {
                    break;
                }
                continue;
            }
            // Discovery observes attribution and Worker authority only. Like
            // Backend discovery, body availability remains unchecked until a
            // bounded search/read; listing never scans every Session body.
            let availability = self.authorize(attribution.clone());
            if availability
                .as_ref()
                .err()
                .is_some_and(|e| e.code == SubjektivSessionDiagnosticCode::NotFoundOrNotAuthorized)
            {
                if selector.is_some() {
                    return Err(denied());
                }
                cursor.after = Some(after);
                if !has_more {
                    break;
                }
                continue;
            }
            // Standalone FsStore has no archive catalog. Archived filtering must
            // not manufacture an archive or fall back to an unrelated root.
            if input.storage == SubjektivSessionStorageFilter::Archived {
                cursor.after = Some(after);
                if !has_more {
                    break;
                }
                continue;
            }
            let item = SubjektivSessionListItem {
                session_id: attribution.session_id,
                attributed_at: attribution.attributed_at,
                storage: SubjektivSessionStorage::Retained,
                availability: if availability.is_ok() {
                    SubjektivSessionAvailability::Unchecked
                } else {
                    SubjektivSessionAvailability::Unavailable
                },
                reason: availability.err().map(|e| e.message),
            };
            items.push(item);
            if json_len(&items) > ITEM_BUDGET {
                items.pop();
                if items.is_empty() {
                    return Err(resource_limit());
                }
                has_more = true;
                break;
            }
            cursor.after = Some(after);
            if items.len() >= limit || !has_more {
                break;
            }
        }
        let next_cursor = has_more.then(|| encode_cursor(&cursor)).transpose()?;
        Ok(SubjektivSessionBackendResult::List(
            SubjektivSessionListResponse {
                items,
                next_cursor,
                has_more,
            },
        ))
    }

    fn search(
        &self,
        input: SubjektivSessionSearchRequest,
    ) -> LocalResult<SubjektivSessionBackendResult> {
        let limit = limit(input.limit)?;
        let selector = optional_selector(input.session_id.as_deref())?;
        let tool_name = optional_selector(input.tool_name.as_deref())?;
        let query = input.query.as_deref().unwrap_or("").trim();
        if query.len() > 4096 || (query.is_empty() && selector.is_none()) {
            return Err(failure(
                SubjektivSessionDiagnosticCode::InvalidInput,
                "Supply a bounded query, or select a Session to list its public entries",
            ));
        }
        // Explicit foreign selectors are denied even if the query has no hits.
        if let Some(id) = selector {
            self.authorize(self.attribution(id)?)?;
        }
        let mut cursor = self.cursor(
            "search",
            serde_json::json!({"session_id": selector, "query": query,
            "kind": input.kind, "tool_name": tool_name, "tool_part": input.tool_part}),
            input.cursor.as_deref(),
        )?;
        if cursor.byte_offset != 0 {
            return Err(stale());
        }
        let mut items = Vec::new();
        let mut issues = Vec::new();
        let mut has_more = false;
        let mut scan_limits = limits();
        'sessions: for _ in 0..MAX_SCANNED_SESSIONS {
            let page = self.page(&mut cursor, selector)?;
            let more_attributions = page.has_more && selector.is_none();
            let Some(attribution) = page.items.into_iter().next() else {
                if cursor.current.is_some() {
                    return Err(stale());
                }
                break;
            };
            let after = (
                attribution.attributed_at.clone(),
                attribution.session_id.clone(),
            );
            let source = self.authorize(attribution.clone());
            let index = source.and_then(|source| self.index(&source, scan_limits));
            let index = match index {
                Ok(index) => index,
                Err(error) => {
                    if cursor.current.is_some() {
                        return Err(error);
                    }
                    if error.code == SubjektivSessionDiagnosticCode::ResourceLimit
                        && (scan_limits.max_bytes < MAX_SCAN_BYTES
                            || scan_limits.max_entries < MAX_SCAN_ENTRIES
                            || scan_limits.max_segments < MAX_SCAN_SEGMENTS)
                    {
                        // This Session may fit a fresh call. Do not consume its
                        // attribution just because earlier Sessions used budget.
                        has_more = true;
                        break;
                    }
                    let exhausted_scan = matches!(
                        error.code,
                        SubjektivSessionDiagnosticCode::ResourceLimit
                            | SubjektivSessionDiagnosticCode::CorruptLog
                            | SubjektivSessionDiagnosticCode::RuntimeUnavailable
                    );
                    if error.code != SubjektivSessionDiagnosticCode::NotFoundOrNotAuthorized {
                        issues.push(SubjektivSessionIssue {
                            session_id: attribution.session_id.clone(),
                            code: error.code,
                            message: error.message,
                        });
                    } else if selector.is_some() {
                        return Err(denied());
                    }
                    cursor.after = Some(after);
                    has_more = more_attributions;
                    if exhausted_scan || issues.len() >= 20 || !has_more {
                        break;
                    }
                    continue;
                }
            };
            scan_limits.max_bytes = scan_limits.max_bytes.saturating_sub(index.scanned_bytes);
            scan_limits.max_entries = scan_limits
                .max_entries
                .saturating_sub(index.scanned_entries.max(index.entry_count));
            scan_limits.max_segments = scan_limits
                .max_segments
                .saturating_sub(index.segments.len());
            let mut offset = 0;
            if let Some(position) = &cursor.current {
                if position.session_id != attribution.session_id
                    || position.worker_id != attribution.worker_id
                    || position.generation != index.generation
                    || position.entry_offset > index.entry_count
                {
                    return Err(stale());
                }
                offset = position.entry_offset;
            }
            let total = index
                .segments
                .iter()
                .map(|s| s.entries.len())
                .sum::<usize>();
            let mut entry_offset = offset;
            for (segment, entry) in index
                .segments
                .iter()
                .flat_map(|s| s.entries.iter().map(move |e| (s, e)))
                .skip(offset)
            {
                if entry_matches(entry, query, input.kind, tool_name, input.tool_part) {
                    let (snippet, snippet_truncated) = snippet(&entry.full_text, query);
                    items.push(SubjektivSessionSearchItem {
                        session_id: index.session_id.clone(),
                        segment_id: segment.segment_id.clone(),
                        entry_ref: entry.entry_ref.clone(),
                        kind: entry_kind(entry.kind),
                        origin: entry.provenance.clone(),
                        tool_name: entry.tool_name.clone(),
                        tool_part: entry.tool_part.map(tool_part),
                        snippet,
                        snippet_truncated,
                        lineage: lineage(&segment.lineage),
                    });
                    if json_len(&items) > ITEM_BUDGET {
                        items.pop();
                        if items.is_empty() {
                            return Err(resource_limit());
                        }
                        cursor.current = Some(Position {
                            session_id: attribution.session_id.clone(),
                            worker_id: attribution.worker_id.clone(),
                            generation: index.generation.clone(),
                            entry_offset,
                        });
                        has_more = true;
                        break 'sessions;
                    }
                }
                entry_offset += 1;
                if items.len() >= limit && entry_offset < total {
                    cursor.current = Some(Position {
                        session_id: attribution.session_id.clone(),
                        worker_id: attribution.worker_id.clone(),
                        generation: index.generation.clone(),
                        entry_offset,
                    });
                    has_more = true;
                    break 'sessions;
                }
            }
            cursor.current = None;
            cursor.after = Some(after);
            has_more = more_attributions;
            if items.len() >= limit || !has_more {
                break;
            }
            if scan_limits.max_bytes == 0
                || scan_limits.max_entries == 0
                || scan_limits.max_segments == 0
            {
                break;
            }
        }
        let next_cursor = has_more.then(|| encode_cursor(&cursor)).transpose()?;
        let coverage = if issues.is_empty() {
            SubjektivSessionCoverage::Complete
        } else {
            SubjektivSessionCoverage::Partial
        };
        Ok(SubjektivSessionBackendResult::Search(
            SubjektivSessionSearchResponse {
                items,
                issues,
                coverage,
                next_cursor,
                has_more,
            },
        ))
    }

    fn read(
        &self,
        input: SubjektivSessionReadRequest,
    ) -> LocalResult<SubjektivSessionBackendResult> {
        required_selector(&input.session_id)?;
        required_selector(&input.segment_id)?;
        required_selector(&input.entry_ref)?;
        let mut cursor = self.cursor(
            "read",
            serde_json::json!({"session_id": input.session_id,
            "segment_id": input.segment_id, "entry_ref": input.entry_ref, "mode": input.mode}),
            input.cursor.as_deref(),
        )?;
        if cursor.after.is_some() {
            return Err(stale());
        }
        let attribution = self.attribution(&input.session_id)?;
        let source = self.authorize(attribution)?;
        let index = self.index(&source, limits())?;
        if let Some(position) = &cursor.current {
            if position.session_id != input.session_id
                || position.worker_id != source.attribution.worker_id
                || position.generation != index.generation
                || position.entry_offset != 0
            {
                return Err(stale());
            }
        } else if input.cursor.is_some() {
            return Err(stale());
        }
        let segment = index
            .segments
            .iter()
            .find(|s| s.segment_id == input.segment_id)
            .ok_or_else(denied)?;
        let entry = segment
            .entries
            .iter()
            .find(|e| e.entry_ref == input.entry_ref)
            .ok_or_else(denied)?;
        let text = match input.mode {
            SubjektivSessionReadMode::Compact => &entry.compact_text,
            SubjektivSessionReadMode::Full => &entry.full_text,
        };
        if cursor.byte_offset > text.len() || !text.is_char_boundary(cursor.byte_offset) {
            return Err(stale());
        }
        cursor.current = Some(Position {
            session_id: input.session_id.clone(),
            worker_id: source.attribution.worker_id,
            generation: index.generation.clone(),
            entry_offset: 0,
        });
        let start = cursor.byte_offset;
        let mut max_bytes = SUBJEKTIV_SESSION_MAX_READ_CONTENT_BYTES;
        loop {
            let end = chunk_end(text, start, max_bytes, MAX_LINES);
            let has_more = end < text.len();
            cursor.byte_offset = end;
            let next_cursor = has_more.then(|| encode_cursor(&cursor)).transpose()?;
            let result = SubjektivSessionBackendResult::Read(SubjektivSessionReadResponse {
                session_id: input.session_id.clone(),
                segment_id: input.segment_id.clone(),
                entry_ref: input.entry_ref.clone(),
                kind: entry_kind(entry.kind),
                origin: entry.provenance.clone(),
                lineage: lineage(&segment.lineage),
                mode: input.mode,
                content: text[start..end].into(),
                truncated: has_more
                    || (input.mode == SubjektivSessionReadMode::Compact
                        && entry.compact_text_truncated),
                has_more,
                next_cursor,
            });
            if fits(&SubjektivSessionBackendResponse::Ok {
                result: result.clone(),
            }) {
                return Ok(result);
            }
            if max_bytes <= 4 {
                return Err(resource_limit());
            }
            max_bytes = (max_bytes / 2).max(4);
        }
    }
}

fn limits() -> SessionPublicIndexLimits {
    SessionPublicIndexLimits {
        max_bytes: MAX_SCAN_BYTES,
        max_segments: MAX_SCAN_SEGMENTS,
        max_entries: MAX_SCAN_ENTRIES,
    }
}

fn failure(code: SubjektivSessionDiagnosticCode, message: &str) -> Failure {
    Failure {
        code,
        message: message.into(),
    }
}
fn denied() -> Failure {
    failure(
        SubjektivSessionDiagnosticCode::NotFoundOrNotAuthorized,
        "Session entry was not found or is not authorized",
    )
}
fn stale() -> Failure {
    failure(
        SubjektivSessionDiagnosticCode::StaleCursor,
        "Session cursor is invalid or no longer matches its scope, filters, or committed generation",
    )
}
fn subject_unavailable() -> Failure {
    failure(
        SubjektivSessionDiagnosticCode::SubjectUnavailable,
        "Bound Subject storage is unavailable",
    )
}
fn resource_limit() -> Failure {
    failure(
        SubjektivSessionDiagnosticCode::ResourceLimit,
        "Session operation exceeds its bounded resource budget",
    )
}
fn error_response(error: Failure) -> SubjektivSessionBackendResponse {
    SubjektivSessionBackendResponse::Error { error }
}
fn index_failure(error: SessionPublicIndexReadError) -> Failure {
    let code = match error {
        SessionPublicIndexReadError::MigrationRequired => {
            SubjektivSessionDiagnosticCode::MigrationRequired
        }
        SessionPublicIndexReadError::Missing => SubjektivSessionDiagnosticCode::RetentionMissing,
        SessionPublicIndexReadError::Corrupt => SubjektivSessionDiagnosticCode::CorruptLog,
        SessionPublicIndexReadError::Storage => SubjektivSessionDiagnosticCode::RuntimeUnavailable,
        SessionPublicIndexReadError::ResourceLimit => SubjektivSessionDiagnosticCode::ResourceLimit,
        SessionPublicIndexReadError::StaleCursor => SubjektivSessionDiagnosticCode::StaleCursor,
    };
    failure(code, &error.to_string())
}
fn limit(value: Option<usize>) -> LocalResult<usize> {
    let value = value.unwrap_or(20);
    if (1..=100).contains(&value) {
        Ok(value)
    } else {
        Err(failure(
            SubjektivSessionDiagnosticCode::InvalidInput,
            "limit must be between 1 and 100",
        ))
    }
}
fn required_selector(value: &str) -> LocalResult<()> {
    if value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        Err(failure(
            SubjektivSessionDiagnosticCode::InvalidInput,
            "Session selectors must be nonempty bounded identifiers",
        ))
    } else {
        Ok(())
    }
}
fn optional_selector(value: Option<&str>) -> LocalResult<Option<&str>> {
    value.map(required_selector).transpose()?;
    Ok(value)
}
fn json_len(value: &impl Serialize) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |v| v.len())
}
fn fits(value: &SubjektivSessionBackendResponse) -> bool {
    json_len(value) <= SUBJEKTIV_SESSION_MAX_TOOL_CONTENT_BYTES
}

// Hex is only an opaque transport encoding, not an authorization credential.
// Every decoded identity is rechecked against durable Host authority on use.
fn encode_cursor(cursor: &Cursor) -> LocalResult<String> {
    let bytes = serde_json::to_vec(cursor).map_err(|_| resource_limit())?;
    if CURSOR_PREFIX.len() + bytes.len() * 2 > MAX_CURSOR_BYTES {
        return Err(resource_limit());
    }
    let mut token = String::with_capacity(CURSOR_PREFIX.len() + bytes.len() * 2);
    token.push_str(CURSOR_PREFIX);
    const HEX: &[u8] = b"0123456789abcdef";
    for byte in bytes {
        token.push(HEX[(byte >> 4) as usize] as char);
        token.push(HEX[(byte & 15) as usize] as char);
    }
    Ok(token)
}
fn decode_cursor(token: &str) -> LocalResult<Cursor> {
    if token.len() > MAX_CURSOR_BYTES {
        return Err(stale());
    }
    let hex = token.strip_prefix(CURSOR_PREFIX).ok_or_else(stale)?;
    if hex.len() % 2 != 0 {
        return Err(stale());
    }
    fn nibble(c: u8) -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            _ => None,
        }
    }
    let bytes = hex
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| Some(nibble(pair[0])? * 16 + nibble(pair[1])?))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(stale)?;
    serde_json::from_slice(&bytes).map_err(|_| stale())
}
fn entry_kind(kind: SessionPublicIndexEntryKind) -> SubjektivSessionEntryKind {
    match kind {
        SessionPublicIndexEntryKind::User => SubjektivSessionEntryKind::User,
        SessionPublicIndexEntryKind::Assistant => SubjektivSessionEntryKind::Assistant,
        SessionPublicIndexEntryKind::Tool => SubjektivSessionEntryKind::Tool,
    }
}
fn tool_part(part: SessionPublicIndexToolPart) -> SubjektivSessionToolPart {
    match part {
        SessionPublicIndexToolPart::Input => SubjektivSessionToolPart::Input,
        SessionPublicIndexToolPart::Output => SubjektivSessionToolPart::Output,
    }
}
fn lineage(value: &SessionPublicIndexLineage) -> SubjektivSessionLineage {
    SubjektivSessionLineage {
        kind: match value.origin_kind {
            SessionPublicIndexOriginKind::Root => SubjektivSessionLineageKind::Root,
            SessionPublicIndexOriginKind::Fork => SubjektivSessionLineageKind::Fork,
            SessionPublicIndexOriginKind::Compact => SubjektivSessionLineageKind::Compact,
        },
        parent_segment_id: value.parent_segment_id.clone(),
        at_turn_index: value.parent_turn_index,
    }
}
fn entry_matches(
    entry: &SessionPublicIndexEntry,
    query: &str,
    kind: Option<SubjektivSessionEntryKind>,
    name: Option<&str>,
    part: SubjektivSessionToolPart,
) -> bool {
    kind.is_none_or(|k| k == entry_kind(entry.kind))
        && name.is_none_or(|name| entry.tool_name.as_deref() == Some(name))
        && (entry.tool_part.is_none()
            || part == SubjektivSessionToolPart::Both
            || entry.tool_part.map(tool_part) == Some(part))
        && (query.is_empty()
            || entry
                .full_text
                .to_lowercase()
                .contains(&query.to_lowercase()))
}
fn snippet(text: &str, query: &str) -> (String, bool) {
    // Search matching is case-insensitive; snippet framing uses exact matches
    // only to avoid byte offsets into case-folded Unicode text.
    let mut start = if query.is_empty() {
        0
    } else {
        text.find(query).unwrap_or(0).saturating_sub(128)
    };
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    let end = chunk_end(text, start, 512, 8);
    (text[start..end].into(), start > 0 || end < text.len())
}
fn chunk_end(text: &str, start: usize, max_bytes: usize, max_lines: usize) -> usize {
    let mut end = start.saturating_add(max_bytes).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    if let Some((offset, _)) = text[start..end].match_indices('\n').nth(max_lines - 1) {
        end = start + offset + 1;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::StaleLeasePolicy;
    use crate::subjektiv::StandaloneSubjectBinding;
    use feature_storage::FeatureStorage;
    use session_store::SessionPublicIndexSegment;

    // Test-only committed public index fixtures exercise backend authorization
    // and paging independently of storage layout. The production callback must
    // be the shared strict reader (whose run-boundary tests live in session-store).
    fn fixture_reader(
        root: &Path,
        id: SessionId,
        bounds: SessionPublicIndexLimits,
    ) -> Result<SessionPublicIndex, SessionPublicIndexReadError> {
        let bytes = std::fs::read(root.join(format!("{id}.fixture"))).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                SessionPublicIndexReadError::Missing
            } else {
                SessionPublicIndexReadError::Storage
            }
        })?;
        let index: SessionPublicIndex =
            serde_json::from_slice(&bytes).map_err(|_| SessionPublicIndexReadError::Corrupt)?;
        if index.scanned_bytes > bounds.max_bytes
            || index.scanned_entries > bounds.max_entries
            || index.segments.len() > bounds.max_segments
        {
            return Err(SessionPublicIndexReadError::ResourceLimit);
        }
        Ok(index)
    }

    struct Fixture {
        temp: tempfile::TempDir,
        store: SubjektivStore,
        _manager: FeatureStorage,
        subject: String,
        worker: WorkerId,
        session: SessionId,
        segment: String,
        index_path: PathBuf,
    }

    impl Fixture {
        fn new(texts: &[&str]) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let manager = FeatureStorage::new(temp.path().join("features"));
            let scope = manager.workspace("local-scope-a").unwrap();
            let registration = SubjektivStore::register(&scope).unwrap();
            let store = SubjektivStore::open(&scope, &registration).unwrap();
            let subject = store
                .create_subject(subjektiv::SubjectRole::new("standalone_subject").unwrap())
                .unwrap()
                .id;
            let workers = StandaloneWorkerStore::open(temp.path().join("workers")).unwrap();
            let allocation = workers
                .allocate(temp.path(), StaleLeasePolicy::Reject)
                .unwrap();
            let session = SessionId::now_v7();
            let segment = SessionId::now_v7().to_string();
            let manifest = manifest::WorkerManifest::from_toml(
                r#"
[worker]
name = "session-test"
[model]
scheme = "anthropic"
model_id = "claude-sonnet-4-20250514"
[engine]
[scope]
allow = []
"#,
            )
            .unwrap();
            workers
                .commit_created_connected(
                    &allocation,
                    manifest,
                    "session-test".into(),
                    session,
                    None,
                    Some(StandaloneSubjectBinding {
                        scope_id: "local-scope-a".into(),
                        subject_id: subject.clone(),
                    }),
                )
                .unwrap();
            let worker = allocation.worker_id();
            store
                .record_session_attribution(
                    SubjectSessionAttribution::new(
                        &subject,
                        "local-scope-a",
                        worker.to_string(),
                        session.to_string(),
                    )
                    .unwrap(),
                )
                .unwrap();
            let index_path = workers
                .sessions_dir(worker)
                .join(format!("{session}.fixture"));
            let index = SessionPublicIndex {
                session_id: session.to_string(),
                generation: "generation-1".into(),
                scanned_bytes: 100,
                scanned_entries: texts.len(),
                entry_count: texts.len(),
                segments: vec![SessionPublicIndexSegment {
                    segment_id: segment.clone(),
                    lineage: SessionPublicIndexLineage {
                        origin_kind: SessionPublicIndexOriginKind::Root,
                        parent_segment_id: None,
                        parent_turn_index: None,
                    },
                    entries: texts
                        .iter()
                        .enumerate()
                        .map(|(i, text)| SessionPublicIndexEntry {
                            segment_id: segment.clone(),
                            entry_ref: format!("E{i}"),
                            kind: SessionPublicIndexEntryKind::User,
                            provenance: protocol::SessionEntryProvenance::HumanInput,
                            timestamp: 1,
                            tool_part: None,
                            tool_name: None,
                            compact_text: text.chars().take(100).collect(),
                            compact_text_truncated: text.len() > 100,
                            full_text: (*text).into(),
                        })
                        .collect(),
                }],
            };
            std::fs::write(&index_path, serde_json::to_vec(&index).unwrap()).unwrap();
            Self {
                temp,
                store,
                _manager: manager,
                subject,
                worker,
                session,
                segment,
                index_path,
            }
        }
        fn execute(
            &self,
            operation: SubjektivSessionBackendOperation,
        ) -> SubjektivSessionBackendResponse {
            execute_with_reader(
                &self.temp.path().join("workers"),
                "local-scope-a",
                &self.subject,
                &self.store,
                operation,
                fixture_reader,
            )
            .unwrap()
        }
        fn list(&self, cursor: Option<String>, limit: usize) -> SubjektivSessionBackendResponse {
            self.execute(SubjektivSessionBackendOperation::List(
                SubjektivSessionListRequest {
                    session_id: None,
                    storage: SubjektivSessionStorageFilter::All,
                    cursor,
                    limit: Some(limit),
                },
            ))
        }
        fn search(&self, cursor: Option<String>, limit: usize) -> SubjektivSessionBackendResponse {
            self.execute(SubjektivSessionBackendOperation::Search(
                SubjektivSessionSearchRequest {
                    query: None,
                    session_id: Some(self.session.to_string()),
                    kind: None,
                    tool_name: None,
                    tool_part: SubjektivSessionToolPart::Both,
                    cursor,
                    limit: Some(limit),
                },
            ))
        }
        fn read(&self, cursor: Option<String>) -> SubjektivSessionBackendResponse {
            self.execute(SubjektivSessionBackendOperation::Read(
                SubjektivSessionReadRequest {
                    session_id: self.session.to_string(),
                    segment_id: self.segment.clone(),
                    entry_ref: "E0".into(),
                    mode: SubjektivSessionReadMode::Full,
                    cursor,
                },
            ))
        }
    }

    fn expect_error(
        response: SubjektivSessionBackendResponse,
        code: SubjektivSessionDiagnosticCode,
    ) {
        match response {
            SubjektivSessionBackendResponse::Error { error } => assert_eq!(error.code, code),
            _ => panic!("expected {code:?}, got {response:?}"),
        }
    }

    #[test]
    fn discovers_sessions_without_staging_candidates_and_empty_subjects() {
        let f = Fixture::new(&["public experience"]);
        match f.list(None, 20) {
            SubjektivSessionBackendResponse::Ok {
                result: SubjektivSessionBackendResult::List(page),
            } => {
                assert_eq!(page.items.len(), 1);
                assert_eq!(page.items[0].session_id, f.session.to_string());
                assert_eq!(
                    page.items[0].availability,
                    SubjektivSessionAvailability::Unchecked
                );
                assert!(!page.has_more);
            }
            response => panic!("{response:?}"),
        }
        let other = f
            .store
            .create_subject(subjektiv::SubjectRole::new("other_subject").unwrap())
            .unwrap();
        let response = execute_with_reader(
            &f.temp.path().join("workers"),
            "local-scope-a",
            &other.id,
            &f.store,
            SubjektivSessionBackendOperation::List(SubjektivSessionListRequest {
                session_id: None,
                storage: SubjektivSessionStorageFilter::All,
                limit: None,
                cursor: None,
            }),
            fixture_reader,
        )
        .unwrap();
        match response {
            SubjektivSessionBackendResponse::Ok {
                result: SubjektivSessionBackendResult::List(page),
            } => {
                assert!(page.items.is_empty());
                assert!(!page.has_more);
            }
            response => panic!("{response:?}"),
        }
    }

    #[test]
    fn foreign_subject_and_storage_scope_reads_are_denied_even_for_known_ids() {
        let f = Fixture::new(&["secret from first subject"]);
        let other = f
            .store
            .create_subject(subjektiv::SubjectRole::new("other_subject").unwrap())
            .unwrap();
        let operation = SubjektivSessionBackendOperation::Read(SubjektivSessionReadRequest {
            session_id: f.session.to_string(),
            segment_id: f.segment.clone(),
            entry_ref: "E0".into(),
            mode: SubjektivSessionReadMode::Full,
            cursor: None,
        });
        for (scope, subject) in [
            ("local-scope-a", other.id.as_str()),
            ("foreign-scope", f.subject.as_str()),
        ] {
            expect_error(
                execute_with_reader(
                    &f.temp.path().join("workers"),
                    scope,
                    subject,
                    &f.store,
                    operation.clone(),
                    fixture_reader,
                )
                .unwrap(),
                SubjektivSessionDiagnosticCode::NotFoundOrNotAuthorized,
            );
        }
        let backend = Backend {
            state_root: &f.temp.path().join("workers"),
            scope_id: "local-scope-a",
            subject_id: &f.subject,
            store: &f.store,
            reader: fixture_reader,
        };
        let mut attribution = f
            .store
            .session_attribution(&f.session.to_string())
            .unwrap()
            .unwrap();
        attribution.worker_id = WorkerId::now_v7().to_string();
        assert_eq!(
            backend.authorize(attribution).err().unwrap().code,
            SubjektivSessionDiagnosticCode::RetentionMissing
        );
    }

    #[test]
    fn mismatched_worker_subject_binding_is_denied_without_reading_body() {
        let f = Fixture::new(&["must not leak"]);
        let record_path = f
            .temp
            .path()
            .join("workers")
            .join(f.worker.to_string())
            .join("record.json");
        let mut record: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
        record["subject"]["subject_id"] = serde_json::json!("foreign-subject");
        std::fs::write(record_path, serde_json::to_vec(&record).unwrap()).unwrap();
        // Corrupt the body as well: authorization must still win over parsing.
        std::fs::write(&f.index_path, b"bad index").unwrap();
        expect_error(
            f.read(None),
            SubjektivSessionDiagnosticCode::NotFoundOrNotAuthorized,
        );
        expect_error(
            f.search(None, 20),
            SubjektivSessionDiagnosticCode::NotFoundOrNotAuthorized,
        );
        expect_error(
            f.execute(SubjektivSessionBackendOperation::List(
                SubjektivSessionListRequest {
                    session_id: Some(f.session.to_string()),
                    storage: SubjektivSessionStorageFilter::All,
                    limit: None,
                    cursor: None,
                },
            )),
            SubjektivSessionDiagnosticCode::NotFoundOrNotAuthorized,
        );
    }

    #[test]
    fn search_continuations_have_no_duplicate_or_missing_refs_and_bind_filters() {
        let f = Fixture::new(&["first", "second", "third"]);
        let mut cursor = None;
        let mut refs = Vec::new();
        loop {
            let response = f.search(cursor, 1);
            assert!(fits(&response));
            match response {
                SubjektivSessionBackendResponse::Ok {
                    result: SubjektivSessionBackendResult::Search(page),
                } => {
                    refs.extend(page.items.into_iter().map(|item| item.entry_ref));
                    if !page.has_more {
                        assert!(page.next_cursor.is_none());
                        break;
                    }
                    cursor = page.next_cursor;
                }
                response => panic!("{response:?}"),
            }
        }
        assert_eq!(refs, ["E0", "E1", "E2"]);
        let token = match f.search(None, 1) {
            SubjektivSessionBackendResponse::Ok {
                result: SubjektivSessionBackendResult::Search(page),
            } => page.next_cursor.unwrap(),
            response => panic!("{response:?}"),
        };
        let mut decoded = decode_cursor(&token).unwrap();
        decoded.scope_id = "foreign-scope".into();
        expect_error(
            f.search(Some(encode_cursor(&decoded).unwrap()), 1),
            SubjektivSessionDiagnosticCode::StaleCursor,
        );
        expect_error(
            f.execute(SubjektivSessionBackendOperation::Search(
                SubjektivSessionSearchRequest {
                    query: Some("changed query".into()),
                    session_id: Some(f.session.to_string()),
                    kind: None,
                    tool_name: None,
                    tool_part: SubjektivSessionToolPart::Both,
                    cursor: Some(token),
                    limit: Some(1),
                },
            )),
            SubjektivSessionDiagnosticCode::StaleCursor,
        );
    }

    #[test]
    fn read_continuations_bound_utf8_lines_and_serialized_json_bytes() {
        let text = format!("{}{}", "\0\"\\🦀".repeat(6000), "line\n".repeat(700));
        let f = Fixture::new(&[&text]);
        let mut cursor = None;
        let mut reconstructed = String::new();
        loop {
            let response = f.read(cursor);
            assert!(fits(&response));
            match response {
                SubjektivSessionBackendResponse::Ok {
                    result: SubjektivSessionBackendResult::Read(page),
                } => {
                    assert!(page.content.len() <= SUBJEKTIV_SESSION_MAX_READ_CONTENT_BYTES);
                    assert!(page.content.matches('\n').count() <= MAX_LINES);
                    assert!(!page.content.is_empty());
                    reconstructed.push_str(&page.content);
                    if !page.has_more {
                        assert!(page.next_cursor.is_none());
                        break;
                    }
                    assert!(page.truncated);
                    cursor = page.next_cursor;
                }
                response => panic!("{response:?}"),
            }
        }
        assert_eq!(reconstructed, text);
    }

    #[test]
    fn read_and_search_cursor_generation_changes_are_explicit() {
        let text = "long".repeat(6000);
        let f = Fixture::new(&[&text, "second"]);
        let read_cursor = match f.read(None) {
            SubjektivSessionBackendResponse::Ok {
                result: SubjektivSessionBackendResult::Read(page),
            } => page.next_cursor.unwrap(),
            response => panic!("{response:?}"),
        };
        let search_cursor = match f.search(None, 1) {
            SubjektivSessionBackendResponse::Ok {
                result: SubjektivSessionBackendResult::Search(page),
            } => page.next_cursor.unwrap(),
            response => panic!("{response:?}"),
        };
        let mut index: SessionPublicIndex =
            serde_json::from_slice(&std::fs::read(&f.index_path).unwrap()).unwrap();
        index.generation = "new-generation".into();
        std::fs::write(&f.index_path, serde_json::to_vec(&index).unwrap()).unwrap();
        expect_error(
            f.read(Some(read_cursor)),
            SubjektivSessionDiagnosticCode::StaleCursor,
        );
        expect_error(
            f.search(Some(search_cursor), 1),
            SubjektivSessionDiagnosticCode::StaleCursor,
        );
    }

    #[test]
    fn missing_and_corrupt_sessions_have_explicit_diagnostics_and_partial_search() {
        let f = Fixture::new(&["public"]);
        std::fs::remove_file(&f.index_path).unwrap();
        expect_error(
            f.read(None),
            SubjektivSessionDiagnosticCode::RetentionMissing,
        );
        match f.search(None, 20) {
            SubjektivSessionBackendResponse::Ok {
                result: SubjektivSessionBackendResult::Search(page),
            } => {
                assert!(page.items.is_empty());
                assert_eq!(page.coverage, SubjektivSessionCoverage::Partial);
                assert_eq!(
                    page.issues[0].code,
                    SubjektivSessionDiagnosticCode::RetentionMissing
                );
            }
            response => panic!("{response:?}"),
        }
        std::fs::write(&f.index_path, b"corrupt").unwrap();
        expect_error(f.read(None), SubjektivSessionDiagnosticCode::CorruptLog);
        expect_error(
            f.execute(SubjektivSessionBackendOperation::Read(
                SubjektivSessionReadRequest {
                    session_id: "known-but-absent".into(),
                    segment_id: f.segment.clone(),
                    entry_ref: "E0".into(),
                    mode: SubjektivSessionReadMode::Full,
                    cursor: None,
                },
            )),
            SubjektivSessionDiagnosticCode::NotFoundOrNotAuthorized,
        );
    }

    fn worker_tree(root: &Path) -> Vec<(PathBuf, Vec<u8>, std::time::SystemTime)> {
        fn collect(
            root: &Path,
            path: &Path,
            output: &mut Vec<(PathBuf, Vec<u8>, std::time::SystemTime)>,
        ) {
            let metadata = std::fs::metadata(path).unwrap();
            if metadata.is_dir() {
                output.push((
                    path.strip_prefix(root).unwrap().into(),
                    Vec::new(),
                    metadata.modified().unwrap(),
                ));
                for entry in std::fs::read_dir(path).unwrap() {
                    collect(root, &entry.unwrap().path(), output);
                }
            } else {
                output.push((
                    path.strip_prefix(root).unwrap().into(),
                    std::fs::read(path).unwrap(),
                    metadata.modified().unwrap(),
                ));
            }
        }
        let mut output = Vec::new();
        collect(root, root, &mut output);
        output.sort_by(|a, b| a.0.cmp(&b.0));
        output
    }

    #[test]
    fn discovery_keyset_pagination_is_bounded_and_excludes_later_attributions() {
        let f = Fixture::new(&["public"]);
        let mut expected = std::collections::BTreeSet::from([f.session.to_string()]);
        for _ in 0..137 {
            let session = SessionId::now_v7().to_string();
            let mut attribution = SubjectSessionAttribution::new(
                &f.subject,
                "local-scope-a",
                f.worker.to_string(),
                &session,
            )
            .unwrap();
            attribution.attributed_at = "2020-01-01T00:00:00.000Z".into();
            f.store.record_session_attribution(attribution).unwrap();
            expected.insert(session);
        }
        let before = worker_tree(&f.temp.path().join("workers"));
        let mut cursor = None;
        let mut found = Vec::new();
        loop {
            let response = f.list(cursor, 100);
            assert!(fits(&response));
            match response {
                SubjektivSessionBackendResponse::Ok {
                    result: SubjektivSessionBackendResult::List(page),
                } => {
                    assert!(page.items.len() <= 100);
                    found.extend(page.items.into_iter().map(|item| item.session_id));
                    if !page.has_more {
                        assert!(page.next_cursor.is_none());
                        break;
                    }
                    if cursor_is_first(&found) {
                        let mut later = SubjectSessionAttribution::new(
                            &f.subject,
                            "local-scope-a",
                            f.worker.to_string(),
                            SessionId::now_v7().to_string(),
                        )
                        .unwrap();
                        later.attributed_at = "2099-01-01T00:00:00.000Z".into();
                        f.store.record_session_attribution(later).unwrap();
                    }
                    cursor = page.next_cursor;
                }
                response => panic!("{response:?}"),
            }
        }
        assert_eq!(found.len(), expected.len());
        assert_eq!(
            found.into_iter().collect::<std::collections::BTreeSet<_>>(),
            expected
        );
        assert_eq!(before, worker_tree(&f.temp.path().join("workers")));

        fn cursor_is_first(found: &[String]) -> bool {
            found.len() <= 100
        }
    }

    #[test]
    fn missing_state_root_observation_never_creates_a_directory() {
        let f = Fixture::new(&["public"]);
        let absent = f.temp.path().join("absent-state-root");
        expect_error(
            execute(
                &absent,
                "local-scope-a",
                &f.subject,
                &f.store,
                SubjektivSessionBackendOperation::Read(SubjektivSessionReadRequest {
                    session_id: f.session.to_string(),
                    segment_id: f.segment.clone(),
                    entry_ref: "E0".into(),
                    mode: SubjektivSessionReadMode::Full,
                    cursor: None,
                }),
            )
            .unwrap(),
            SubjektivSessionDiagnosticCode::RetentionMissing,
        );
        assert!(!absent.exists());
    }

    #[test]
    fn real_fs_projection_hides_system_reasoning_and_unfinished_or_partial_records() {
        use session_store::{
            LogEntry, LoggedContentPart, LoggedHistoryEntry, LoggedItem, LoggedRole,
            LoggedSessionHistoryEntryId, LoggedSessionHistoryMetadata, LoggedSessionHistoryOrigin,
            Store,
        };
        use std::io::Write;

        let f = Fixture::new(&[]);
        let history = |id: &str, item: LoggedItem| LoggedHistoryEntry {
            item,
            metadata: LoggedSessionHistoryMetadata {
                entry_id: LoggedSessionHistoryEntryId(id.into()),
                origin: LoggedSessionHistoryOrigin::BackendInstruction { operation_id: None },
                derivation: None,
            },
        };
        let message = |id: &str, role: LoggedRole, text: &str| {
            history(
                id,
                LoggedItem::Message {
                    role,
                    content: vec![LoggedContentPart::Text { text: text.into() }],
                },
            )
        };
        let sessions = f.index_path.parent().unwrap();
        let fs_store = session_store::FsStore::new(sessions).unwrap();
        let segment: SessionId = f.segment.parse().unwrap();
        fs_store
            .create_segment(
                f.session,
                segment,
                &[
                    LogEntry::AnnotatedSegmentStart {
                        ts: 1,
                        session_id: f.session,
                        system_prompt: Some("hidden-system-prompt".into()),
                        config: Default::default(),
                        forked_from: None,
                        compacted_from: None,
                        history: vec![
                            message("public", LoggedRole::User, "visible committed input"),
                            message("system", LoggedRole::System, "hidden-system-message"),
                            history(
                                "reasoning",
                                LoggedItem::Reasoning {
                                    text: "hidden-reasoning".into(),
                                    summary: vec!["hidden-reasoning-summary".into()],
                                    encrypted_content: None,
                                    signature: None,
                                },
                            ),
                        ],
                    },
                    LogEntry::Invoke {
                        ts: 2,
                        trigger: protocol::InvokeKind::UserSend,
                    },
                    LogEntry::AnnotatedAssistantItem {
                        ts: 3,
                        entry: message(
                            "unfinished",
                            LoggedRole::Assistant,
                            "hidden-unfinished-output",
                        ),
                    },
                ],
            )
            .unwrap();
        let path = sessions
            .join(f.session.to_string())
            .join(format!("{segment}.jsonl"));
        let mut log = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        // A valid JSON record without its final newline is not committed.
        log.write_all(
            &serde_json::to_vec(&LogEntry::RunCompleted {
                ts: 4,
                interrupted: false,
                result: agen::EngineResult::Finished,
                active_run_turn_count: None,
            })
            .unwrap(),
        )
        .unwrap();
        drop(log);
        let operation = SubjektivSessionBackendOperation::Search(SubjektivSessionSearchRequest {
            query: None,
            session_id: Some(f.session.to_string()),
            kind: None,
            tool_name: None,
            tool_part: SubjektivSessionToolPart::Both,
            limit: None,
            cursor: None,
        });
        let before = worker_tree(&f.temp.path().join("workers"));
        let response = execute(
            &f.temp.path().join("workers"),
            "local-scope-a",
            &f.subject,
            &f.store,
            operation.clone(),
        )
        .unwrap();
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("visible committed input"));
        assert!(!json.contains("hidden-"));
        match response {
            SubjektivSessionBackendResponse::Ok {
                result: SubjektivSessionBackendResult::Search(page),
            } => {
                assert_eq!(page.items.len(), 1);
                assert!(page.issues.is_empty());
            }
            response => panic!("{response:?}"),
        }
        let read = execute(
            &f.temp.path().join("workers"),
            "local-scope-a",
            &f.subject,
            &f.store,
            SubjektivSessionBackendOperation::Read(SubjektivSessionReadRequest {
                session_id: f.session.to_string(),
                segment_id: f.segment.clone(),
                entry_ref: "Epublic".into(),
                mode: SubjektivSessionReadMode::Full,
                cursor: None,
            }),
        )
        .unwrap();
        assert!(matches!(
            read,
            SubjektivSessionBackendResponse::Ok {
                result: SubjektivSessionBackendResult::Read(_)
            }
        ));
        assert!(fits(&f.list(None, 100)));
        assert_eq!(before, worker_tree(&f.temp.path().join("workers")));

        // Commit the completion: the same shared projection now exposes only
        // the public assistant output, never hidden seed content.
        let mut log = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        log.write_all(b"\n").unwrap();
        drop(log);
        let response = execute(
            &f.temp.path().join("workers"),
            "local-scope-a",
            &f.subject,
            &f.store,
            operation,
        )
        .unwrap();
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("hidden-unfinished-output"));
        assert!(!json.contains("hidden-system"));
        assert!(!json.contains("hidden-reasoning"));
    }

    #[test]
    fn evidence_validation_uses_same_subject_authority() {
        let f = Fixture::new(&["evidence"]);
        let index = authorized_index_with_reader(
            &f.temp.path().join("workers"),
            "local-scope-a",
            &f.subject,
            &f.store,
            &f.session.to_string(),
            fixture_reader,
        )
        .unwrap();
        assert_eq!(index.segments[0].entries[0].entry_ref, "E0");
        assert!(
            authorized_index_with_reader(
                &f.temp.path().join("workers"),
                "local-scope-a",
                "foreign-subject",
                &f.store,
                &f.session.to_string(),
                fixture_reader
            )
            .is_err()
        );
    }
}
