//! Filesystem store for the canonical `1 Worker = 1 Session` aggregate.
//!
//! Layout under one Worker aggregate:
//! - `session/session.json` — immutable Session identity
//! - `session/segments/<segment_id>.jsonl`
//! - `session/segments/<segment_id>.trace.jsonl`
//!
//! Unlike [`crate::FsStore`], this store cannot enumerate or switch between
//! arbitrary Sessions. The first segment materializes the sole Session identity;
//! every later operation must use that same ID.

use crate::event_trace::TraceEntry;
use crate::segment_log::LogEntry;
use crate::store::{Store, StoreError};
use crate::{
    LoggedHistoryEntry, LoggedItem, LoggedSessionHistoryEntryId, LoggedSessionHistoryMetadata,
    LoggedSessionHistoryOrigin, LoggedSystemHistoryEntry, SegmentId, SessionId,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

const SESSION_SCHEMA_VERSION: u32 = 3;
const PREVIOUS_SESSION_SCHEMA_VERSION: u32 = 2;
const LEGACY_SESSION_SCHEMA_VERSION: u32 = 1;
const SESSION_FILE: &str = "session.json";
const SEGMENTS_DIR: &str = "segments";

#[derive(Clone)]
pub struct WorkerSessionStore {
    root: PathBuf,
    session_id: Arc<Mutex<Option<SessionId>>>,
    append_lock: Arc<Mutex<()>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SessionManifest {
    schema_version: u32,
    session_id: SessionId,
}

impl WorkerSessionStore {
    /// Open the Session store rooted at `<worker-aggregate>/session`.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        fs::create_dir_all(root.join(SEGMENTS_DIR))?;
        let session_id = match fs::read(root.join(SESSION_FILE)) {
            Ok(bytes) => {
                let mut manifest: SessionManifest = serde_json::from_slice(&bytes)?;
                match manifest.schema_version {
                    SESSION_SCHEMA_VERSION => {
                        validate_canonical_segment_logs(&root)?;
                    }
                    PREVIOUS_SESSION_SCHEMA_VERSION | LEGACY_SESSION_SCHEMA_VERSION => {
                        migrate_segment_logs_to_v3(&root, manifest.session_id)?;
                        manifest.schema_version = SESSION_SCHEMA_VERSION;
                        atomic_write_json(&root.join(SESSION_FILE), &manifest)?;
                    }
                    version => {
                        return Err(StoreError::Corrupt {
                            line: 0,
                            message: format!(
                                "unsupported Worker Session schema version {version}, expected {SESSION_SCHEMA_VERSION}"
                            ),
                        });
                    }
                }
                Some(manifest.session_id)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            root,
            session_id: Arc::new(Mutex::new(session_id)),
            append_lock: Arc::new(Mutex::new(())),
        })
    }

    pub fn root_dir(&self) -> &Path {
        &self.root
    }

    pub fn session_id(&self) -> Result<Option<SessionId>, StoreError> {
        self.session_id
            .lock()
            .map(|session_id| *session_id)
            .map_err(|_| std::io::Error::other("Worker Session identity lock was poisoned").into())
    }

    pub fn session_modified_at(&self) -> Result<Option<SystemTime>, StoreError> {
        let metadata = match fs::metadata(&self.root) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut latest = Some(metadata.modified()?);
        for entry in fs::read_dir(self.root.join(SEGMENTS_DIR))? {
            let modified = entry?.metadata()?.modified()?;
            if latest.map(|current| modified > current).unwrap_or(true) {
                latest = Some(modified);
            }
        }
        Ok(latest)
    }

    fn ensure_session(&self, requested: SessionId, materialize: bool) -> Result<(), StoreError> {
        let mut session_id = self
            .session_id
            .lock()
            .map_err(|_| std::io::Error::other("Worker Session identity lock was poisoned"))?;
        match *session_id {
            Some(existing) if existing == requested => Ok(()),
            Some(existing) => Err(StoreError::Corrupt {
                line: 0,
                message: format!(
                    "Worker aggregate owns Session {existing}; cannot attach or switch to Session {requested}"
                ),
            }),
            None if !materialize => Err(StoreError::Corrupt {
                line: 0,
                message: format!(
                    "Worker aggregate has no materialized Session; requested Session {requested}"
                ),
            }),
            None => {
                let manifest = SessionManifest {
                    schema_version: SESSION_SCHEMA_VERSION,
                    session_id: requested,
                };
                atomic_write_json(&self.root.join(SESSION_FILE), &manifest)?;
                *session_id = Some(requested);
                Ok(())
            }
        }
    }

    fn log_path(&self, segment_id: SegmentId) -> PathBuf {
        self.root
            .join(SEGMENTS_DIR)
            .join(format!("{segment_id}.jsonl"))
    }

    fn trace_path(&self, segment_id: SegmentId) -> PathBuf {
        self.root
            .join(SEGMENTS_DIR)
            .join(format!("{segment_id}.trace.jsonl"))
    }

    fn append_log_entry(
        &self,
        path: &Path,
        session_id: SessionId,
        segment_id: SegmentId,
        entry: &LogEntry,
    ) -> Result<(), StoreError> {
        let _guard = self
            .append_lock
            .lock()
            .map_err(|_| std::io::Error::other("Worker Session append lock was poisoned"))?;
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .append(true)
            .open(path)?;
        let committed_len = truncate_uncommitted_tail(&mut file)?;
        file.seek(SeekFrom::Start(0))?;
        let mut existing = Vec::new();
        file.read_to_end(&mut existing)?;
        let line_index = parse_jsonl::<LogEntry>(&existing)?.len();
        let entry = canonicalize_log_entry(session_id, segment_id, line_index, entry.clone());
        let line = serde_json::to_string(&entry)?;
        let mut record = Vec::with_capacity(line.len() + 1);
        record.extend_from_slice(line.as_bytes());
        record.push(b'\n');
        if let Err(write_error) = file.write_all(&record) {
            return match file.set_len(committed_len) {
                Ok(()) => Err(write_error.into()),
                Err(rollback_error) => Err(std::io::Error::new(
                    rollback_error.kind(),
                    format!(
                        "session append failed ({write_error}) and rollback failed: {rollback_error}"
                    ),
                )
                .into()),
            };
        }
        Ok(())
    }

    fn append_line(&self, path: &Path, line: &str) -> Result<(), StoreError> {
        let _guard = self
            .append_lock
            .lock()
            .map_err(|_| std::io::Error::other("Worker Session append lock was poisoned"))?;
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .append(true)
            .open(path)?;
        let committed_len = truncate_uncommitted_tail(&mut file)?;
        let mut record = Vec::with_capacity(line.len() + 1);
        record.extend_from_slice(line.as_bytes());
        record.push(b'\n');
        if let Err(write_error) = file.write_all(&record) {
            return match file.set_len(committed_len) {
                Ok(()) => Err(write_error.into()),
                Err(rollback_error) => Err(std::io::Error::new(
                    rollback_error.kind(),
                    format!(
                        "session append failed ({write_error}) and rollback failed: {rollback_error}"
                    ),
                )
                .into()),
            };
        }
        Ok(())
    }
}

impl Store for WorkerSessionStore {
    fn append(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        entry: &LogEntry,
    ) -> Result<(), StoreError> {
        self.ensure_session(session_id, true)?;
        self.append_log_entry(&self.log_path(segment_id), session_id, segment_id, entry)
    }

    fn read_all(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
    ) -> Result<Vec<LogEntry>, StoreError> {
        self.ensure_session(session_id, false)?;
        let path = self.log_path(segment_id);
        if !path.exists() {
            return Err(StoreError::NotFound(segment_id));
        }
        parse_jsonl(&fs::read(path)?)
    }

    fn list_sessions(&self) -> Result<Vec<SessionId>, StoreError> {
        Ok(self.session_id()?.into_iter().collect())
    }

    fn list_segments(&self, session_id: SessionId) -> Result<Vec<SegmentId>, StoreError> {
        self.ensure_session(session_id, false)?;
        let mut segments: Vec<SegmentId> = Vec::new();
        for entry in fs::read_dir(self.root.join(SEGMENTS_DIR))? {
            let path = entry?.path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if name.ends_with(".jsonl")
                && !name.ends_with(".trace.jsonl")
                && let Ok(segment_id) = name.trim_end_matches(".jsonl").parse()
            {
                segments.push(segment_id);
            }
        }
        segments.sort_by(|left, right| right.cmp(left));
        Ok(segments)
    }

    fn lookup_session_of(&self, segment_id: SegmentId) -> Result<Option<SessionId>, StoreError> {
        let session_id = self.session_id()?;
        Ok(session_id.filter(|_| self.log_path(segment_id).exists()))
    }

    fn create_segment(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        entries: &[LogEntry],
    ) -> Result<(), StoreError> {
        self.ensure_session(session_id, true)?;
        let mut content = Vec::new();
        for (line_index, entry) in entries.iter().enumerate() {
            let entry = canonicalize_log_entry(session_id, segment_id, line_index, entry.clone());
            serde_json::to_writer(&mut content, &entry)?;
            content.push(b'\n');
        }
        atomic_write_bytes(&self.log_path(segment_id), &content)?;
        Ok(())
    }

    fn exists(&self, session_id: SessionId, segment_id: SegmentId) -> Result<bool, StoreError> {
        self.ensure_session(session_id, false)?;
        Ok(self.log_path(segment_id).exists())
    }

    fn read_entry_count(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
    ) -> Result<usize, StoreError> {
        self.ensure_session(session_id, false)?;
        let path = self.log_path(segment_id);
        if !path.exists() {
            return Err(StoreError::NotFound(segment_id));
        }
        let content = fs::read(path)?;
        let complete = complete_jsonl_prefix(&content);
        let complete = std::str::from_utf8(complete).map_err(|error| StoreError::Corrupt {
            line: complete[..error.valid_up_to()]
                .iter()
                .filter(|byte| **byte == b'\n')
                .count()
                + 1,
            message: error.to_string(),
        })?;
        Ok(complete
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count())
    }

    fn append_trace(
        &self,
        session_id: SessionId,
        segment_id: SegmentId,
        entry: &TraceEntry,
    ) -> Result<(), StoreError> {
        self.ensure_session(session_id, true)?;
        self.append_line(&self.trace_path(segment_id), &serde_json::to_string(entry)?)
    }
}

fn segment_log_paths(root: &Path) -> Result<Vec<(SegmentId, PathBuf)>, StoreError> {
    let segments = root.join(SEGMENTS_DIR);
    if !segments.exists() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    for entry in fs::read_dir(&segments)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return Err(StoreError::Corrupt {
                line: 0,
                message: format!("non-UTF-8 Worker Session segment path: {}", path.display()),
            });
        };
        if name.ends_with(".trace.jsonl") || name.starts_with('.') {
            continue;
        }
        if !name.ends_with(".jsonl") {
            continue;
        }
        if !metadata.file_type().is_file() {
            return Err(StoreError::Corrupt {
                line: 0,
                message: format!(
                    "Worker Session segment is not a regular file: {}",
                    path.display()
                ),
            });
        }
        let segment_id =
            name.trim_end_matches(".jsonl")
                .parse()
                .map_err(|_| StoreError::Corrupt {
                    line: 0,
                    message: format!("invalid Worker Session segment name: {name}"),
                })?;
        paths.push((segment_id, path));
    }
    paths.sort_by_key(|(segment_id, _)| *segment_id);
    Ok(paths)
}

fn migrate_segment_logs_to_v3(root: &Path, session_id: SessionId) -> Result<(), StoreError> {
    for (segment_id, path) in segment_log_paths(root)? {
        let source = fs::read(&path)?;
        let entries: Vec<LogEntry> = parse_jsonl(&source).map_err(|error| StoreError::Corrupt {
            line: 0,
            message: format!(
                "cannot migrate Worker Session log {}: {error}",
                path.display()
            ),
        })?;
        let canonical = entries
            .into_iter()
            .enumerate()
            .map(|(line_index, entry)| {
                canonicalize_log_entry(session_id, segment_id, line_index, entry)
            })
            .collect::<Vec<_>>();
        validate_canonical_entries(&path, &canonical)?;
        let mut output = Vec::new();
        for entry in canonical {
            serde_json::to_writer(&mut output, &entry)?;
            output.push(b'\n');
        }

        // Opening a Session is the exclusive restore boundary, but retain an
        // unchanged-source fence so a racing writer cannot be silently lost.
        if fs::read(&path)? != source {
            return Err(StoreError::Corrupt {
                line: 0,
                message: format!(
                    "Worker Session segment changed during migration: {}",
                    path.display()
                ),
            });
        }
        atomic_write_bytes(&path, &output)?;
    }
    Ok(())
}

fn validate_canonical_segment_logs(root: &Path) -> Result<(), StoreError> {
    for (_, path) in segment_log_paths(root)? {
        let entries: Vec<LogEntry> = parse_jsonl(&fs::read(&path)?)?;
        validate_canonical_entries(&path, &entries)?;
    }
    Ok(())
}

fn validate_canonical_entries(path: &Path, entries: &[LogEntry]) -> Result<(), StoreError> {
    for (line_index, entry) in entries.iter().enumerate() {
        if matches!(
            entry,
            LogEntry::SegmentStart { .. }
                | LogEntry::UserInput { .. }
                | LogEntry::AssistantItem { .. }
                | LogEntry::ToolResult { .. }
                | LogEntry::SystemItem { .. }
        ) {
            return Err(StoreError::Corrupt {
                line: line_index + 1,
                message: format!(
                    "Worker Session schema v3 contains legacy history record in {}",
                    path.display()
                ),
            });
        }
    }
    Ok(())
}

fn legacy_metadata(
    _session_id: SessionId,
    segment_id: SegmentId,
    line_index: usize,
    item_index: usize,
) -> LoggedSessionHistoryMetadata {
    let mut identity = Vec::with_capacity(32);
    identity.extend_from_slice(segment_id.as_bytes());
    identity.extend_from_slice(&(line_index as u64).to_be_bytes());
    identity.extend_from_slice(&(item_index as u64).to_be_bytes());
    LoggedSessionHistoryMetadata {
        entry_id: LoggedSessionHistoryEntryId(format!("l-{}", URL_SAFE_NO_PAD.encode(identity))),
        origin: LoggedSessionHistoryOrigin::LegacyUnknown,
        derivation: None,
    }
}

fn canonicalize_log_entry(
    session_id: SessionId,
    segment_id: SegmentId,
    line_index: usize,
    entry: LogEntry,
) -> LogEntry {
    match entry {
        LogEntry::SegmentStart {
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
                    metadata: legacy_metadata(session_id, segment_id, line_index, item_index),
                })
                .collect(),
            forked_from,
            compacted_from,
        },
        LogEntry::UserInput {
            ts,
            segments,
            extensions,
        } => LogEntry::AnnotatedUserInput {
            ts,
            history: vec![LoggedHistoryEntry {
                item: LoggedItem::from(agen::Item::user_message(
                    protocol::Segment::flatten_to_text(&segments),
                )),
                metadata: legacy_metadata(session_id, segment_id, line_index, 0),
            }],
            segments,
            extensions,
        },
        LogEntry::AssistantItem { ts, item } => LogEntry::AnnotatedAssistantItem {
            ts,
            entry: LoggedHistoryEntry {
                item,
                metadata: legacy_metadata(session_id, segment_id, line_index, 0),
            },
        },
        LogEntry::ToolResult { ts, item } => LogEntry::AnnotatedToolResult {
            ts,
            entry: LoggedHistoryEntry {
                item,
                metadata: legacy_metadata(session_id, segment_id, line_index, 0),
            },
        },
        LogEntry::SystemItem { ts, item } => LogEntry::AnnotatedSystemItem {
            ts,
            entry: LoggedSystemHistoryEntry {
                item,
                metadata: legacy_metadata(session_id, segment_id, line_index, 0),
            },
        },
        canonical => canonical,
    }
}

fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), StoreError> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    atomic_write_bytes(path, &bytes)
}

fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("Worker Session path has no parent"))?;
    fs::create_dir_all(parent)?;
    let tmp = path.with_file_name(format!(
        ".{}.tmp-{}-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("session"),
        std::process::id(),
        uuid::Uuid::now_v7()
    ));
    let result = (|| -> Result<(), StoreError> {
        let mut file = OpenOptions::new().create_new(true).write(true).open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn complete_jsonl_prefix(content: &[u8]) -> &[u8] {
    if content.last() == Some(&b'\n') {
        return content;
    }
    content
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map(|index| &content[..=index])
        .unwrap_or(&[])
}

fn parse_jsonl<T: serde::de::DeserializeOwned>(content: &[u8]) -> Result<Vec<T>, StoreError> {
    let complete = complete_jsonl_prefix(content);
    let content = std::str::from_utf8(complete).map_err(|error| StoreError::Corrupt {
        line: complete[..error.valid_up_to()]
            .iter()
            .filter(|byte| **byte == b'\n')
            .count()
            + 1,
        message: error.to_string(),
    })?;
    content
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str(line).map_err(|error| StoreError::Corrupt {
                line: index + 1,
                message: error.to_string(),
            })
        })
        .collect()
}

fn truncate_uncommitted_tail(file: &mut File) -> std::io::Result<u64> {
    const SCAN_BYTES: usize = 8 * 1024;
    let len = file.metadata()?.len();
    if len == 0 {
        return Ok(0);
    }
    file.seek(SeekFrom::End(-1))?;
    let mut last = [0_u8; 1];
    file.read_exact(&mut last)?;
    if last[0] == b'\n' {
        return Ok(len);
    }
    let mut end = len;
    let mut buffer = [0_u8; SCAN_BYTES];
    while end > 0 {
        let start = end.saturating_sub(SCAN_BYTES as u64);
        let chunk_len = (end - start) as usize;
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut buffer[..chunk_len])?;
        if let Some(index) = buffer[..chunk_len].iter().rposition(|byte| *byte == b'\n') {
            let committed_len = start + index as u64 + 1;
            file.set_len(committed_len)?;
            return Ok(committed_len);
        }
        end = start;
    }
    file.set_len(0)?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Store, new_segment_id, new_session_id};

    #[test]
    fn canonical_layout_and_single_session_invariant() {
        let root = tempfile::tempdir().unwrap();
        let store = WorkerSessionStore::new(root.path().join("session")).unwrap();
        let session_id = new_session_id();
        let segment_id = new_segment_id();
        store.create_segment(session_id, segment_id, &[]).unwrap();

        assert!(root.path().join("session/session.json").is_file());
        assert!(
            root.path()
                .join(format!("session/segments/{segment_id}.jsonl"))
                .is_file()
        );
        assert_eq!(store.list_sessions().unwrap(), vec![session_id]);

        let other = new_session_id();
        let error = store
            .create_segment(other, new_segment_id(), &[])
            .unwrap_err();
        assert!(error.to_string().contains("cannot attach or switch"));
        assert_eq!(store.list_sessions().unwrap(), vec![session_id]);
    }

    #[test]
    fn schema_v1_logs_are_rewritten_and_promoted_to_v3() {
        let root = tempfile::tempdir().unwrap();
        let session_id = new_session_id();
        let segment_id = new_segment_id();
        WorkerSessionStore::new(root.path())
            .unwrap()
            .create_segment(session_id, segment_id, &[])
            .unwrap();
        let manifest_path = root.path().join(SESSION_FILE);
        let mut manifest: SessionManifest =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        manifest.schema_version = LEGACY_SESSION_SCHEMA_VERSION;
        atomic_write_json(&manifest_path, &manifest).unwrap();

        let reopened = WorkerSessionStore::new(root.path()).unwrap();
        assert_eq!(reopened.session_id().unwrap(), Some(session_id));
        let migrated: SessionManifest =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        assert_eq!(migrated.schema_version, SESSION_SCHEMA_VERSION);
    }

    #[test]
    fn schema_v1_migration_rejects_corrupt_log_before_v3_manifest_update() {
        let root = tempfile::tempdir().unwrap();
        let session_id = new_session_id();
        let manifest = SessionManifest {
            schema_version: LEGACY_SESSION_SCHEMA_VERSION,
            session_id,
        };
        atomic_write_json(&root.path().join(SESSION_FILE), &manifest).unwrap();
        fs::create_dir_all(root.path().join(SEGMENTS_DIR)).unwrap();
        fs::write(
            root.path().join(SEGMENTS_DIR).join("broken.jsonl"),
            "{not-json}\n",
        )
        .unwrap();

        let error = match WorkerSessionStore::new(root.path()) {
            Ok(_) => panic!("corrupt legacy Session log must reject migration"),
            Err(error) => error,
        };
        assert!(matches!(error, StoreError::Corrupt { .. }));
        let persisted: SessionManifest =
            serde_json::from_slice(&fs::read(root.path().join(SESSION_FILE)).unwrap()).unwrap();
        assert_eq!(persisted.schema_version, LEGACY_SESSION_SCHEMA_VERSION);
    }

    #[test]
    fn schema_v2_migration_rewrites_legacy_records_with_stable_unknown_provenance() {
        let root = tempfile::tempdir().unwrap();
        let session_id = new_session_id();
        let segment_id = new_segment_id();
        fs::create_dir_all(root.path().join(SEGMENTS_DIR)).unwrap();
        atomic_write_json(
            &root.path().join(SESSION_FILE),
            &SessionManifest {
                schema_version: PREVIOUS_SESSION_SCHEMA_VERSION,
                session_id,
            },
        )
        .unwrap();
        let source = vec![
            LogEntry::SegmentStart {
                ts: 1,
                session_id,
                system_prompt: None,
                config: agen::llm_client::RequestConfig::default(),
                history: vec![LoggedItem::from(agen::Item::assistant_message("prior"))],
                forked_from: None,
                compacted_from: None,
            },
            LogEntry::UserInput {
                ts: 2,
                segments: vec![protocol::Segment::Text {
                    content: "hello".into(),
                }],
                extensions: Vec::new(),
            },
            LogEntry::AssistantItem {
                ts: 3,
                item: LoggedItem::from(agen::Item::assistant_message("reply")),
            },
        ];
        let path = root
            .path()
            .join(SEGMENTS_DIR)
            .join(format!("{segment_id}.jsonl"));
        let mut bytes = Vec::new();
        for entry in source {
            serde_json::to_writer(&mut bytes, &entry).unwrap();
            bytes.push(b'\n');
        }
        fs::write(&path, bytes).unwrap();

        let store = WorkerSessionStore::new(root.path()).unwrap();
        let first = store.read_all(session_id, segment_id).unwrap();
        assert!(matches!(first[0], LogEntry::AnnotatedSegmentStart { .. }));
        assert!(matches!(first[1], LogEntry::AnnotatedUserInput { .. }));
        assert!(matches!(first[2], LogEntry::AnnotatedAssistantItem { .. }));
        let first_bytes = fs::read(&path).unwrap();
        drop(store);

        let reopened = WorkerSessionStore::new(root.path()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), first_bytes);
        let snapshot = crate::public_snapshot::project_current_session_snapshot(
            &reopened.read_all(session_id, segment_id).unwrap(),
        );
        assert_eq!(snapshot.entries.len(), 3);
        assert!(snapshot.entries.iter().all(|entry| {
            entry.provenance == protocol::SessionEntryProvenance::LegacyUnknown
                && entry.entry_id.len() <= 64
        }));
    }

    #[test]
    fn schema_v3_rejects_legacy_records_and_new_writes_are_canonical() {
        let root = tempfile::tempdir().unwrap();
        let session_id = new_session_id();
        let segment_id = new_segment_id();
        let store = WorkerSessionStore::new(root.path()).unwrap();
        store
            .create_segment(
                session_id,
                segment_id,
                &[LogEntry::SegmentStart {
                    ts: 1,
                    session_id,
                    system_prompt: None,
                    config: agen::llm_client::RequestConfig::default(),
                    history: Vec::new(),
                    forked_from: None,
                    compacted_from: None,
                }],
            )
            .unwrap();
        store
            .append(
                session_id,
                segment_id,
                &LogEntry::UserInput {
                    ts: 2,
                    segments: vec![protocol::Segment::Text {
                        content: "new".into(),
                    }],
                    extensions: Vec::new(),
                },
            )
            .unwrap();
        let entries = store.read_all(session_id, segment_id).unwrap();
        assert!(matches!(entries[0], LogEntry::AnnotatedSegmentStart { .. }));
        assert!(matches!(entries[1], LogEntry::AnnotatedUserInput { .. }));
        drop(store);

        let path = root
            .path()
            .join(SEGMENTS_DIR)
            .join(format!("{segment_id}.jsonl"));
        let mut file = OpenOptions::new().append(true).open(path).unwrap();
        serde_json::to_writer(
            &mut file,
            &LogEntry::SystemItem {
                ts: 3,
                item: crate::SystemItem::LegacyIgnored {
                    slug: "legacy".into(),
                },
            },
        )
        .unwrap();
        file.write_all(b"\n").unwrap();
        let error = match WorkerSessionStore::new(root.path()) {
            Ok(_) => panic!("schema v3 must reject a legacy history record"),
            Err(error) => error,
        };
        assert!(matches!(error, StoreError::Corrupt { .. }));
    }

    #[test]
    fn reopen_preserves_session_and_segment_ids() {
        let root = tempfile::tempdir().unwrap();
        let session_id = new_session_id();
        let segment_id = new_segment_id();
        WorkerSessionStore::new(root.path())
            .unwrap()
            .create_segment(session_id, segment_id, &[])
            .unwrap();

        let reopened = WorkerSessionStore::new(root.path()).unwrap();
        assert_eq!(reopened.session_id().unwrap(), Some(session_id));
        assert!(reopened.exists(session_id, segment_id).unwrap());
    }
}
