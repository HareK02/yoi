use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use client::WorkerClient;
use manifest::paths;
use protocol::{Event, WorkerStatus};
use serde::Deserialize;
use session_store::{FsStore, SegmentId, SessionId};
use session_store::{WorkerActiveSegmentRef, WorkerMetadata, WorkerMetadataStore};

#[derive(Debug, Clone)]
pub(crate) struct WorkerList {
    pub entries: Vec<WorkerListEntry>,
    pub selected_name: Option<String>,
}

impl WorkerList {
    pub(crate) fn from_sources(
        source: WorkerVisibilitySource,
        stored: Vec<StoredWorkerInfo>,
        live: Vec<LiveWorkerInfo>,
        selected_name: Option<String>,
        max_entries: usize,
    ) -> Self {
        let mut entries_by_name: BTreeMap<String, WorkerListEntry> = BTreeMap::new();

        for stored_info in stored {
            let name = stored_info.worker_name.clone();
            entries_by_name
                .entry(name.clone())
                .or_insert_with(|| WorkerListEntry::new(name, source))
                .merge_stored(stored_info);
        }

        for live_info in live {
            let name = live_info.worker_name.clone();
            entries_by_name
                .entry(name.clone())
                .or_insert_with(|| WorkerListEntry::new(name, source))
                .merge_live(live_info);
        }

        let mut entries: Vec<WorkerListEntry> = entries_by_name.into_values().collect();
        for entry in &mut entries {
            entry.finalize();
        }
        entries.sort_by(|a, b| {
            b.has_reachable_live()
                .cmp(&a.has_reachable_live())
                .then_with(|| b.summary.updated_at.cmp(&a.summary.updated_at))
                .then_with(|| a.name.cmp(&b.name))
        });
        entries.truncate(max_entries);

        let selected_name = selected_name
            .filter(|name| entries.iter().any(|entry| entry.name == *name))
            .or_else(|| entries.first().map(|entry| entry.name.clone()));

        Self {
            entries,
            selected_name,
        }
    }

    pub(crate) fn from_workspace_sources(
        source: WorkerVisibilitySource,
        stored: Vec<StoredWorkerInfo>,
        live: Vec<LiveWorkerInfo>,
        selected_name: Option<String>,
        max_entries: usize,
        workspace_root: &Path,
    ) -> Self {
        let current_workspace = workspace_root_key(workspace_root);
        let mut current_names = BTreeSet::new();
        let stored: Vec<_> = stored
            .into_iter()
            .filter(|info| {
                let matches = info
                    .workspace_root
                    .as_deref()
                    .is_some_and(|root| workspace_root_key(root) == current_workspace);
                if matches {
                    current_names.insert(info.worker_name.clone());
                }
                matches
            })
            .collect();
        let live = live
            .into_iter()
            .filter(|info| current_names.contains(&info.worker_name))
            .collect();
        Self::from_sources(source, stored, live, selected_name, max_entries)
    }

    pub(crate) fn filter_for_workspace(&self, workspace_root: &Path) -> Self {
        let current_workspace = workspace_root_key(workspace_root);
        let entries: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| entry_belongs_to_workspace(entry, &current_workspace))
            .cloned()
            .collect();
        let selected_name = self
            .selected_name
            .as_ref()
            .filter(|name| entries.iter().any(|entry| entry.name == **name))
            .cloned()
            .or_else(|| entries.first().map(|entry| entry.name.clone()));
        Self {
            entries,
            selected_name,
        }
    }

    pub(crate) fn selected_index(&self) -> usize {
        self.selected_name
            .as_ref()
            .and_then(|name| self.entries.iter().position(|entry| entry.name == *name))
            .unwrap_or(0)
    }

    pub(crate) fn select_index(&mut self, index: usize) {
        self.selected_name = self.entries.get(index).map(|entry| entry.name.clone());
    }

    pub(crate) fn retain_live_entries(&mut self) {
        self.entries.retain(|entry| entry.live.is_some());
        if !self
            .selected_name
            .as_ref()
            .is_some_and(|selected| self.entries.iter().any(|entry| entry.name == *selected))
        {
            self.selected_name = self.entries.first().map(|entry| entry.name.clone());
        }
    }

    pub(crate) fn selected_entry(&self) -> Option<&WorkerListEntry> {
        let index = self.selected_index();
        self.entries.get(index)
    }
}

fn workspace_root_key(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn entry_belongs_to_workspace(entry: &WorkerListEntry, current_workspace: &Path) -> bool {
    entry
        .stored
        .as_ref()
        .and_then(|stored| stored.workspace_root.as_deref())
        .is_some_and(|root| workspace_root_key(root) == current_workspace)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkerVisibilitySource {
    ResumePicker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkerListSourceKind {
    RuntimeRegistry,
    StoredMetadata,
}

#[derive(Debug, Clone)]
pub(crate) struct WorkerListEntry {
    pub name: String,
    pub visibility: WorkerVisibilitySource,
    pub source_kinds: Vec<WorkerListSourceKind>,
    pub live: Option<LiveWorkerInfo>,
    pub stored: Option<StoredWorkerInfo>,
    pub summary: WorkerEntrySummary,
    pub actions: WorkerEntryActions,
    pub diagnostics: Vec<WorkerEntryDiagnostic>,
}

impl WorkerListEntry {
    fn new(name: String, visibility: WorkerVisibilitySource) -> Self {
        Self {
            name,
            visibility,
            source_kinds: Vec::new(),
            live: None,
            stored: None,
            summary: WorkerEntrySummary::default(),
            actions: WorkerEntryActions::default(),
            diagnostics: Vec::new(),
        }
    }

    fn merge_live(&mut self, live: LiveWorkerInfo) {
        if !self
            .source_kinds
            .contains(&WorkerListSourceKind::RuntimeRegistry)
        {
            self.source_kinds
                .push(WorkerListSourceKind::RuntimeRegistry);
        }
        if live.summary.updated_at > self.summary.updated_at {
            self.summary.updated_at = live.summary.updated_at;
        }
        if self.summary.active_session_id.is_none() {
            self.summary.active_session_id = live.summary.active_session_id;
        }
        if self.summary.active_segment_id.is_none() {
            self.summary.active_segment_id = live.summary.active_segment_id.or(live.segment_id);
        }
        if self.summary.preview.is_none() {
            self.summary.preview = live.summary.preview.clone();
        }
        self.live = Some(live);
    }

    fn merge_stored(&mut self, stored: StoredWorkerInfo) {
        if !self
            .source_kinds
            .contains(&WorkerListSourceKind::StoredMetadata)
        {
            self.source_kinds.push(WorkerListSourceKind::StoredMetadata);
        }
        if stored.updated_at > self.summary.updated_at {
            self.summary.updated_at = stored.updated_at;
        }
        if self.summary.active_session_id.is_none() {
            self.summary.active_session_id = stored.active_session_id;
        }
        if self.summary.active_segment_id.is_none() {
            self.summary.active_segment_id = stored.active_segment_id;
        }
        if self.summary.preview.is_none() {
            self.summary.preview = stored.preview.clone();
        }
        self.stored = Some(stored);
    }

    fn finalize(&mut self) {
        self.fill_live_pending_preview();
        self.diagnostics = build_diagnostics(self);
        self.actions = build_actions(self);
    }

    fn has_reachable_live(&self) -> bool {
        self.live.as_ref().is_some_and(|live| live.reachable)
    }

    fn fill_live_pending_preview(&mut self) {
        if !self.has_reachable_live() || self.summary.updated_at != 0 {
            return;
        }
        let preview_is_pending = self.summary.preview.as_deref() == Some("[pending segment]");
        let preview_is_incomplete = self.summary.preview.is_none() || preview_is_pending;
        if preview_is_incomplete && (self.summary.active_segment_id.is_some() || preview_is_pending)
        {
            self.summary.preview = Some("[live, pending segment]".to_string());
        }
    }

    pub(crate) fn attach_socket_path(&self) -> Option<&Path> {
        self.live
            .as_ref()
            .filter(|live| live.reachable)
            .map(|live| live.socket_path.as_path())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct LiveWorkerInfo {
    pub worker_name: String,
    pub socket_path: PathBuf,
    pub status: Option<WorkerStatus>,
    pub reachable: bool,
    pub segment_id: Option<SegmentId>,
    pub summary: WorkerEntrySummary,
}

#[derive(Debug, Clone)]
pub(crate) struct StoredWorkerInfo {
    pub worker_name: String,
    pub metadata_state: StoredMetadataState,
    pub active_session_id: Option<SessionId>,
    pub active_segment_id: Option<SegmentId>,
    pub updated_at: u64,
    pub workspace_root: Option<PathBuf>,
    pub preview: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StoredMetadataState {
    Present,
    Corrupt(String),
}

#[derive(Debug, Clone, Default)]
pub(crate) struct WorkerEntrySummary {
    pub active_session_id: Option<SessionId>,
    pub active_segment_id: Option<SegmentId>,
    pub updated_at: u64,
    pub preview: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WorkerEntryActions {
    pub can_open: bool,
    pub can_restore: bool,
    pub can_send_now: bool,
    pub can_queue_send: bool,
    pub disabled_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkerEntryDiagnostic {
    pub kind: WorkerEntryDiagnosticKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkerEntryDiagnosticKind {
    StoredMetadataCorrupt,
    LiveUnreachable,
    MissingStoredMetadata,
    MissingLiveStatus,
}

pub(crate) fn read_stored_worker_infos(
    store: &FsStore,
    worker_metadata_store: &impl WorkerMetadataStore,
) -> Result<Vec<StoredWorkerInfo>, io::Error> {
    let mut records = Vec::new();
    for worker_name in worker_metadata_store
        .list_names()
        .map_err(io::Error::other)?
    {
        let info = match worker_metadata_store.read_by_name(&worker_name) {
            Ok(Some(metadata)) => stored_info_from_metadata(store, worker_name, metadata),
            Ok(None) => corrupt_stored_info(
                worker_name,
                "metadata disappeared during discovery".to_string(),
            ),
            Err(e) => corrupt_stored_info(worker_name, e.to_string()),
        };
        records.push(info);
    }
    Ok(records)
}

pub(crate) fn read_live_worker_infos() -> Result<Vec<LiveWorkerInfo>, io::Error> {
    let path = paths::worker_allocation_path().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "could not resolve worker allocation path",
        )
    })?;
    let table = match read_worker_allocation_table(&path) {
        Ok(table) => table,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    Ok(table
        .allocations
        .into_iter()
        .map(|allocation| LiveWorkerInfo {
            worker_name: allocation.worker_name,
            socket_path: allocation.socket,
            status: None,
            reachable: false,
            segment_id: allocation.segment_id,
            summary: WorkerEntrySummary::default(),
        })
        .collect())
}

fn read_worker_allocation_table(path: &Path) -> Result<WorkerAllocationTable, io::Error> {
    let mut file = File::open(path)?;
    fs4::fs_std::FileExt::lock_shared(&file)?;
    let mut contents = String::new();
    let read_result = file.read_to_string(&mut contents);
    let unlock_result = fs4::fs_std::FileExt::unlock(&file);
    read_result?;
    unlock_result?;

    if contents.trim().is_empty() {
        return Ok(WorkerAllocationTable::default());
    }
    serde_json::from_str(&contents).map_err(io::Error::other)
}

#[derive(Debug, Default, Deserialize)]
struct WorkerAllocationTable {
    #[serde(default)]
    allocations: Vec<WorkerAllocationRecord>,
}

#[derive(Debug, Deserialize)]
struct WorkerAllocationRecord {
    worker_name: String,
    socket: PathBuf,
    #[serde(default)]
    segment_id: Option<SegmentId>,
}

pub(crate) async fn read_reachable_live_worker_infos(
    store: &FsStore,
) -> Result<Vec<LiveWorkerInfo>, io::Error> {
    let records = read_live_worker_infos()?;
    probe_reachable_live_worker_infos(store, records).await
}

async fn probe_reachable_live_worker_infos(
    _store: &FsStore,
    records: Vec<LiveWorkerInfo>,
) -> Result<Vec<LiveWorkerInfo>, io::Error> {
    let mut handles = Vec::with_capacity(records.len());
    for record in records {
        handles.push(tokio::spawn(probe_live_worker_info(record)));
    }

    let mut reachable = Vec::with_capacity(handles.len());
    for handle in handles {
        let result = handle
            .await
            .map_err(|e| io::Error::other(format!("live status probe task failed: {e}")))?;
        let Ok(record) = result else {
            continue;
        };
        reachable.push(record);
    }
    Ok(reachable)
}

async fn probe_live_worker_info(mut record: LiveWorkerInfo) -> Result<LiveWorkerInfo, io::Error> {
    let status = probe_live_status(&record.socket_path).await?;
    record.reachable = true;
    record.status = status;
    Ok(record)
}

pub(crate) fn live_socket_for_worker(worker_name: &str) -> Option<PathBuf> {
    read_live_worker_infos()
        .ok()?
        .into_iter()
        .find(|worker| worker.worker_name == worker_name)
        .map(|worker| worker.socket_path)
}

fn stored_info_from_metadata(
    store: &FsStore,
    worker_name: String,
    metadata: WorkerMetadata,
) -> StoredWorkerInfo {
    let active = metadata.active;
    let active_session_id = active.as_ref().map(|a| a.session_id);
    let active_segment_id = active.as_ref().and_then(|a| a.segment_id);
    let summary = summarize_metadata(store, active.as_ref());

    StoredWorkerInfo {
        worker_name,
        metadata_state: StoredMetadataState::Present,
        active_session_id,
        active_segment_id,
        updated_at: summary.updated_at,
        workspace_root: metadata.workspace_root,
        preview: summary.preview,
    }
}

fn corrupt_stored_info(worker_name: String, message: String) -> StoredWorkerInfo {
    StoredWorkerInfo {
        worker_name,
        metadata_state: StoredMetadataState::Corrupt(message.clone()),
        active_session_id: None,
        active_segment_id: None,
        updated_at: 0,
        workspace_root: None,
        preview: Some(format!("metadata: {}", trim_one_line(&message, 48))),
    }
}

const LIVE_STATUS_PROBE_TIMEOUT: Duration = Duration::from_millis(200);

async fn probe_live_status(socket_path: &Path) -> Result<Option<WorkerStatus>, io::Error> {
    let mut client = WorkerClient::connect(socket_path).await?;
    let deadline = tokio::time::Instant::now() + LIVE_STATUS_PROBE_TIMEOUT;

    loop {
        if tokio::time::Instant::now() >= deadline {
            return Ok(None);
        }
        match tokio::time::timeout_at(deadline, client.next_event()).await {
            Ok(Some(event)) => {
                if let Some(status) = status_from_event(&event) {
                    return Ok(Some(status));
                }
            }
            Ok(None) | Err(_) => return Ok(None),
        }
    }
}

fn status_from_event(event: &Event) -> Option<WorkerStatus> {
    match event {
        Event::Snapshot { status, .. } | Event::Status { status } => Some(*status),
        _ => None,
    }
}

#[derive(Debug, Clone)]
struct SegmentSummary {
    updated_at: u64,
    preview: Option<String>,
}

fn summarize_metadata(_store: &FsStore, active: Option<&WorkerActiveSegmentRef>) -> SegmentSummary {
    let Some(active) = active else {
        return SegmentSummary {
            updated_at: 0,
            preview: None,
        };
    };
    match active.segment_id {
        Some(segment_id) => SegmentSummary {
            updated_at: 0,
            preview: Some(format!("active segment {segment_id}")),
        },
        None => SegmentSummary {
            updated_at: 0,
            preview: Some("[pending segment]".to_string()),
        },
    }
}

fn build_diagnostics(entry: &WorkerListEntry) -> Vec<WorkerEntryDiagnostic> {
    let mut diagnostics = Vec::new();

    if let Some(stored) = entry.stored.as_ref() {
        if let StoredMetadataState::Corrupt(message) = &stored.metadata_state {
            diagnostics.push(WorkerEntryDiagnostic {
                kind: WorkerEntryDiagnosticKind::StoredMetadataCorrupt,
                message: format!("metadata: {}", trim_one_line(message, 80)),
            });
        }
    } else if entry.live.is_some() {
        diagnostics.push(WorkerEntryDiagnostic {
            kind: WorkerEntryDiagnosticKind::MissingStoredMetadata,
            message: "no stored worker metadata".to_string(),
        });
    }

    if let Some(live) = entry.live.as_ref() {
        if !live.reachable {
            diagnostics.push(WorkerEntryDiagnostic {
                kind: WorkerEntryDiagnosticKind::LiveUnreachable,
                message: format!("socket unreachable: {}", live.socket_path.display()),
            });
        } else if live.status.is_none() {
            diagnostics.push(WorkerEntryDiagnostic {
                kind: WorkerEntryDiagnosticKind::MissingLiveStatus,
                message: "live worker status was not reported".to_string(),
            });
        }
    }

    diagnostics
}

fn build_actions(entry: &WorkerListEntry) -> WorkerEntryActions {
    let live_reachable = entry.live.as_ref().is_some_and(|live| live.reachable);
    let stored_restorable = entry
        .stored
        .as_ref()
        .is_some_and(|stored| matches!(stored.metadata_state, StoredMetadataState::Present));
    let live_status = entry.live.as_ref().and_then(|live| live.status);

    let can_restore = stored_restorable && !live_reachable;
    let can_open = live_reachable || stored_restorable;
    let can_send_now = live_reachable && live_status == Some(WorkerStatus::Idle);
    let can_queue_send = live_reachable && live_status == Some(WorkerStatus::Running);
    let disabled_reason = if can_open {
        None
    } else if entry.live.is_some() {
        Some("live worker is unreachable".to_string())
    } else if entry.stored.is_some() {
        Some("stored worker metadata is corrupt".to_string())
    } else {
        Some("no live or stored worker state".to_string())
    };

    WorkerEntryActions {
        can_open,
        can_restore,
        can_send_now,
        can_queue_send,
        disabled_reason,
    }
}

fn trim_one_line(s: &str, max_chars: usize) -> String {
    let collapsed: String = s.chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
    if collapsed.chars().count() <= max_chars {
        collapsed
    } else {
        let truncated: String = collapsed.chars().take(max_chars - 1).collect();
        format!("{truncated}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use llm_engine::llm_client::types::RequestConfig;
    use protocol::stream::JsonLineWriter;
    use session_store::FsWorkerStore;
    use session_store::{LogEntry, Store, new_segment_id, new_session_id};
    use session_store::{WorkerActiveSegmentRef, WorkerMetadataStore};
    use tempfile::tempdir;
    use tokio::net::UnixListener;
    use tokio::sync::Barrier;

    const SOURCE: WorkerVisibilitySource = WorkerVisibilitySource::ResumePicker;

    #[test]
    fn stored_metadata_summary_uses_segment_marker_without_reading_session_log() {
        let dir = tempdir().unwrap();
        let store = FsStore::new(dir.path()).unwrap();
        let session = new_session_id();
        let segment = new_segment_id();

        append_start(&store, session, segment, 10);
        append_user(
            &store,
            session,
            segment,
            100,
            "session log text should not be scanned",
        );

        let entry = single_entry(WorkerList::from_sources(
            SOURCE,
            vec![metadata_info(&store, "stored", session, segment)],
            vec![],
            None,
            10,
        ));

        assert_eq!(entry.name, "stored");
        assert_eq!(entry.summary.updated_at, 0);
        assert_eq!(
            entry.summary.preview.as_deref(),
            Some(format!("active segment {segment}").as_str())
        );
    }

    #[test]
    fn reachable_live_rows_sort_before_stopped_rows_before_truncation() {
        let stopped = (0..10)
            .map(|index| stopped_info_with_updated_at(&format!("stopped-{index}"), 1_000 - index))
            .collect::<Vec<_>>();
        let live = live_info_with_updated_at("live-pending", WorkerStatus::Idle, 0);

        let entries = WorkerList::from_sources(SOURCE, stopped, vec![live], None, 10).entries;

        assert_eq!(entries.len(), 10);
        assert_eq!(entries[0].name, "live-pending");
        assert!(entries.iter().all(|entry| entry.name != "stopped-9"));
    }

    #[test]
    fn reachable_live_sort_does_not_promote_unreachable_registry_allocations() {
        let mut unreachable = live_info_with_updated_at("unreachable", WorkerStatus::Idle, 0);
        unreachable.reachable = false;
        unreachable.status = None;

        let entries = WorkerList::from_sources(
            SOURCE,
            vec![stopped_info_with_updated_at("stopped", 100)],
            vec![unreachable],
            None,
            10,
        )
        .entries;

        assert_eq!(entries[0].name, "stopped");
        assert_eq!(entries[1].name, "unreachable");
    }

    #[test]
    fn live_pending_with_runtime_segment_is_attach_only_and_gets_pending_preview() {
        let session_id = new_session_id();
        let runtime_segment_id = new_segment_id();
        let entry = single_entry(WorkerList::from_sources(
            SOURCE,
            vec![pending_metadata_info("pending", session_id)],
            vec![live_info_with_segment(
                "pending",
                WorkerStatus::Idle,
                runtime_segment_id,
            )],
            None,
            10,
        ));

        assert_eq!(entry.name, "pending");
        assert_eq!(entry.summary.active_session_id, Some(session_id));
        assert_eq!(entry.summary.active_segment_id, Some(runtime_segment_id));
        assert_eq!(
            entry.summary.preview.as_deref(),
            Some("[live, pending segment]")
        );
        assert!(entry.actions.can_open);
        assert!(!entry.actions.can_restore);
        assert_eq!(
            entry.attach_socket_path(),
            Some(Path::new("/tmp/pending.sock"))
        );
    }

    #[test]
    fn live_only_runtime_segment_is_attach_only_and_not_restorable() {
        let runtime_segment_id = new_segment_id();
        let entry = single_entry(WorkerList::from_sources(
            SOURCE,
            vec![],
            vec![live_info_with_segment(
                "runtime-only",
                WorkerStatus::Idle,
                runtime_segment_id,
            )],
            None,
            10,
        ));

        assert_eq!(entry.summary.active_segment_id, Some(runtime_segment_id));
        assert_eq!(
            entry.summary.preview.as_deref(),
            Some("[live, pending segment]")
        );
        assert!(entry.actions.can_open);
        assert!(!entry.actions.can_restore);
        assert_eq!(
            entry.attach_socket_path(),
            Some(Path::new("/tmp/runtime-only.sock"))
        );
    }

    #[test]
    fn retain_live_entries_removes_stored_only_rows_and_reselects() {
        let dir = tempdir().unwrap();
        let store = FsStore::new(dir.path()).unwrap();
        let session_id = new_session_id();
        let segment_id = new_segment_id();
        append_start(&store, session_id, segment_id, 10);
        let mut list = WorkerList::from_sources(
            SOURCE,
            vec![metadata_info(&store, "stored", session_id, segment_id)],
            vec![live_info("live", WorkerStatus::Idle)],
            Some("stored".to_string()),
            10,
        );

        list.retain_live_entries();

        assert_eq!(list.entries.len(), 1);
        assert_eq!(list.entries[0].name, "live");
        assert_eq!(list.selected_entry().unwrap().name, "live");
    }

    #[test]
    fn stored_only_row_can_restore_and_open_but_not_direct_send() {
        let dir = tempdir().unwrap();
        let store = FsStore::new(dir.path()).unwrap();
        let session_id = new_session_id();
        let segment_id = new_segment_id();
        append_start(&store, session_id, segment_id, 10);

        let entry = single_entry(WorkerList::from_sources(
            SOURCE,
            vec![metadata_info(&store, "stored", session_id, segment_id)],
            vec![],
            None,
            10,
        ));

        assert_eq!(entry.name, "stored");
        assert_eq!(entry.visibility, SOURCE);
        assert_eq!(
            entry.source_kinds,
            vec![WorkerListSourceKind::StoredMetadata]
        );
        assert!(entry.live.is_none());
        assert!(entry.stored.is_some());
        assert!(entry.actions.can_open);
        assert!(entry.actions.can_restore);
        assert!(!entry.actions.can_send_now);
        assert!(!entry.actions.can_queue_send);
    }

    #[test]
    fn live_idle_reachable_row_can_open_and_send_now() {
        let entry = single_entry(WorkerList::from_sources(
            SOURCE,
            vec![],
            vec![live_info("live", WorkerStatus::Idle)],
            None,
            10,
        ));

        assert_eq!(entry.name, "live");
        assert_eq!(entry.visibility, SOURCE);
        assert_eq!(
            entry.source_kinds,
            vec![WorkerListSourceKind::RuntimeRegistry]
        );
        assert!(entry.actions.can_open);
        assert!(!entry.actions.can_restore);
        assert!(entry.actions.can_send_now);
        assert!(!entry.actions.can_queue_send);
        assert_eq!(
            entry.attach_socket_path(),
            Some(Path::new("/tmp/live.sock"))
        );
    }

    #[test]
    fn live_reachable_row_without_reported_status_can_open_but_not_send_now() {
        let mut live = live_info("live", WorkerStatus::Idle);
        live.status = None;
        live.reachable = true;

        let entry = single_entry(WorkerList::from_sources(
            SOURCE,
            vec![],
            vec![live],
            None,
            10,
        ));

        assert!(entry.actions.can_open);
        assert!(!entry.actions.can_restore);
        assert!(!entry.actions.can_send_now);
        assert!(!entry.actions.can_queue_send);
        assert_eq!(
            entry.attach_socket_path(),
            Some(Path::new("/tmp/live.sock"))
        );
        assert!(
            !entry
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.kind == WorkerEntryDiagnosticKind::LiveUnreachable)
        );
    }

    #[test]
    fn live_running_reachable_row_can_open_but_not_send_now() {
        let entry = single_entry(WorkerList::from_sources(
            SOURCE,
            vec![],
            vec![live_info("live", WorkerStatus::Running)],
            None,
            10,
        ));

        assert!(entry.actions.can_open);
        assert!(!entry.actions.can_restore);
        assert!(!entry.actions.can_send_now);
        assert!(entry.actions.can_queue_send);
    }

    #[test]
    fn live_unreachable_row_has_diagnostic_and_cannot_open() {
        let mut live = live_info("live", WorkerStatus::Idle);
        live.reachable = false;
        live.status = None;

        let entry = single_entry(WorkerList::from_sources(
            SOURCE,
            vec![],
            vec![live],
            None,
            10,
        ));

        assert!(!entry.actions.can_open);
        assert!(!entry.actions.can_restore);
        assert!(!entry.actions.can_send_now);
        assert!(!entry.actions.can_queue_send);
        assert_eq!(
            entry.actions.disabled_reason.as_deref(),
            Some("live worker is unreachable")
        );
        assert_eq!(entry.attach_socket_path(), None);
        assert!(entry.diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == WorkerEntryDiagnosticKind::LiveUnreachable
                && diagnostic.message.contains("/tmp/live.sock")
        }));
    }

    #[test]
    fn status_extraction_skips_alert_before_snapshot() {
        let events = [
            Event::Alert(protocol::Alert {
                level: protocol::AlertLevel::Warn,
                source: protocol::AlertSource::Worker,
                message: "warming up".to_string(),
                timestamp_ms: 0,
            }),
            Event::Snapshot {
                entries: vec![],
                greeting: test_greeting(),
                status: WorkerStatus::Idle,
                in_flight: Default::default(),
            },
        ];

        let status = events.iter().find_map(status_from_event);
        assert_eq!(status, Some(WorkerStatus::Idle));
    }

    #[tokio::test]
    async fn live_status_probes_run_concurrently() {
        let store_dir = tempdir().unwrap();
        let store = FsStore::new(store_dir.path()).unwrap();
        let socket_dir = tempdir().unwrap();
        let probe_count = 3;
        let barrier = Arc::new(Barrier::new(probe_count));
        let mut records = Vec::new();
        let mut servers = Vec::new();

        for index in 0..probe_count {
            let worker_name = format!("worker-{index}");
            let socket_path = socket_dir.path().join(format!("{worker_name}.sock"));
            let listener = UnixListener::bind(&socket_path).unwrap();
            let barrier = Arc::clone(&barrier);
            servers.push(tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                barrier.wait().await;
                let mut writer = JsonLineWriter::new(stream);
                writer
                    .write(&Event::Status {
                        status: WorkerStatus::Idle,
                    })
                    .await
                    .unwrap();
            }));
            records.push(live_probe_record(&worker_name, socket_path));
        }

        let records = tokio::time::timeout(
            LIVE_STATUS_PROBE_TIMEOUT * 3,
            probe_reachable_live_worker_infos(&store, records),
        )
        .await
        .expect("status probes should complete")
        .unwrap();

        assert_eq!(records.len(), probe_count);
        assert!(records.iter().all(|record| record.reachable));
        assert!(
            records
                .iter()
                .all(|record| record.status == Some(WorkerStatus::Idle))
        );
        for server in servers {
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn live_status_probe_timeout_still_marks_socket_reachable() {
        let store_dir = tempdir().unwrap();
        let store = FsStore::new(store_dir.path()).unwrap();
        let socket_dir = tempdir().unwrap();
        let socket_path = socket_dir.path().join("silent.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });

        let records = probe_reachable_live_worker_infos(
            &store,
            vec![live_probe_record("silent", socket_path.clone())],
        )
        .await
        .unwrap();

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].worker_name, "silent");
        assert!(records[0].reachable);
        assert_eq!(records[0].status, None);
        assert_eq!(records[0].socket_path, socket_path);
        server.abort();
    }

    #[test]
    fn corrupt_stored_metadata_has_diagnostic() {
        let entry = single_entry(WorkerList::from_sources(
            SOURCE,
            vec![corrupt_stored_info(
                "broken".to_string(),
                "expected value".to_string(),
            )],
            vec![],
            None,
            10,
        ));

        assert_eq!(entry.name, "broken");
        assert!(!entry.actions.can_open);
        assert!(entry.diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == WorkerEntryDiagnosticKind::StoredMetadataCorrupt
                && diagnostic.message.contains("expected value")
        }));
        assert!(
            entry
                .summary
                .preview
                .as_deref()
                .unwrap()
                .contains("expected value")
        );
    }

    #[test]
    fn selected_worker_name_is_kept_after_rebuild() {
        let first = WorkerList::from_sources(
            SOURCE,
            vec![],
            vec![
                live_info("alpha", WorkerStatus::Idle),
                live_info("beta", WorkerStatus::Idle),
            ],
            Some("alpha".to_string()),
            10,
        );
        assert_eq!(first.selected_entry().unwrap().name, "alpha");

        let rebuilt = WorkerList::from_sources(
            SOURCE,
            vec![],
            vec![
                live_info_with_updated_at("beta", WorkerStatus::Idle, 20),
                live_info_with_updated_at("alpha", WorkerStatus::Idle, 10),
            ],
            first.selected_name.clone(),
            10,
        );

        assert_eq!(rebuilt.entries[0].name, "beta");
        assert_eq!(rebuilt.selected_entry().unwrap().name, "alpha");
        assert_eq!(rebuilt.selected_index(), 1);
    }

    #[test]
    fn read_stored_worker_infos_reports_corrupt_metadata() {
        let dir = tempdir().unwrap();
        let store = FsStore::new(dir.path()).unwrap();
        let worker_metadata_store = FsWorkerStore::new(dir.path().join("workers")).unwrap();
        let worker_metadata_dir = dir.path().join("workers").join("broken");
        std::fs::create_dir_all(&worker_metadata_dir).unwrap();
        std::fs::write(worker_metadata_dir.join("metadata.json"), "{not-json").unwrap();

        let records = read_stored_worker_infos(&store, &worker_metadata_store).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].worker_name, "broken");
        assert!(matches!(
            records[0].metadata_state,
            StoredMetadataState::Corrupt(_)
        ));
    }

    #[test]
    fn read_stored_worker_infos_reads_metadata() {
        let dir = tempdir().unwrap();
        let store = FsStore::new(dir.path()).unwrap();
        let worker_metadata_store = FsWorkerStore::new(dir.path().join("workers")).unwrap();
        let session_id = new_session_id();
        let segment_id = new_segment_id();
        worker_metadata_store
            .write(&WorkerMetadata::new(
                "agent",
                Some(WorkerActiveSegmentRef::active_segment(
                    session_id, segment_id,
                )),
            ))
            .unwrap();

        let records = read_stored_worker_infos(&store, &worker_metadata_store).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].worker_name, "agent");
        assert_eq!(records[0].metadata_state, StoredMetadataState::Present);
    }

    fn single_entry(list: WorkerList) -> WorkerListEntry {
        assert_eq!(list.entries.len(), 1);
        list.entries.into_iter().next().unwrap()
    }

    fn metadata_info(
        store: &FsStore,
        worker_name: &str,
        session_id: SessionId,
        segment_id: SegmentId,
    ) -> StoredWorkerInfo {
        stored_info_from_metadata(
            store,
            worker_name.to_string(),
            WorkerMetadata::new(
                worker_name,
                Some(WorkerActiveSegmentRef::active_segment(
                    session_id, segment_id,
                )),
            ),
        )
    }

    fn pending_metadata_info(worker_name: &str, session_id: SessionId) -> StoredWorkerInfo {
        StoredWorkerInfo {
            worker_name: worker_name.to_string(),
            metadata_state: StoredMetadataState::Present,
            active_session_id: Some(session_id),
            active_segment_id: None,
            updated_at: 0,
            workspace_root: None,
            preview: Some("[pending segment]".to_string()),
        }
    }

    fn stopped_info_with_updated_at(worker_name: &str, updated_at: u64) -> StoredWorkerInfo {
        StoredWorkerInfo {
            worker_name: worker_name.to_string(),
            metadata_state: StoredMetadataState::Present,
            active_session_id: None,
            active_segment_id: None,
            updated_at,
            workspace_root: None,
            preview: None,
        }
    }

    fn live_info(worker_name: &str, status: WorkerStatus) -> LiveWorkerInfo {
        live_info_with_updated_at(worker_name, status, 0)
    }

    fn live_info_with_segment(
        worker_name: &str,
        status: WorkerStatus,
        segment_id: SegmentId,
    ) -> LiveWorkerInfo {
        let mut info = live_info(worker_name, status);
        info.segment_id = Some(segment_id);
        info
    }

    fn live_info_with_updated_at(
        worker_name: &str,
        status: WorkerStatus,
        updated_at: u64,
    ) -> LiveWorkerInfo {
        LiveWorkerInfo {
            worker_name: worker_name.to_string(),
            socket_path: PathBuf::from(format!("/tmp/{worker_name}.sock")),
            status: Some(status),
            reachable: true,
            segment_id: None,
            summary: WorkerEntrySummary {
                active_session_id: None,
                active_segment_id: None,
                updated_at,
                preview: None,
            },
        }
    }

    fn live_probe_record(worker_name: &str, socket_path: PathBuf) -> LiveWorkerInfo {
        LiveWorkerInfo {
            worker_name: worker_name.to_string(),
            socket_path,
            status: None,
            reachable: false,
            segment_id: None,
            summary: WorkerEntrySummary::default(),
        }
    }

    fn test_greeting() -> protocol::Greeting {
        protocol::Greeting {
            worker_name: "live".to_string(),
            cwd: "/tmp".to_string(),
            provider: "test".to_string(),
            model: "test".to_string(),
            scope_summary: "test".to_string(),
            tools: vec![],
            context_window: 0,
            context_tokens: 0,
        }
    }

    fn append_start(store: &FsStore, session_id: SessionId, segment_id: SegmentId, ts: u64) {
        store
            .append(
                session_id,
                segment_id,
                &LogEntry::SegmentStart {
                    ts,
                    session_id,
                    system_prompt: None,
                    config: RequestConfig::default(),
                    history: vec![],
                    forked_from: None,
                    compacted_from: None,
                },
            )
            .unwrap();
    }

    fn append_user(
        store: &FsStore,
        session_id: SessionId,
        segment_id: SegmentId,
        ts: u64,
        text: &str,
    ) {
        store
            .append(
                session_id,
                segment_id,
                &LogEntry::UserInput {
                    ts,
                    segments: vec![protocol::Segment::text(text)],
                },
            )
            .unwrap();
    }

    fn stopped_info_for_workspace(worker_name: &str, workspace_root: &Path) -> StoredWorkerInfo {
        let mut info = stopped_info_with_updated_at(worker_name, 10);
        info.workspace_root = Some(workspace_root.to_path_buf());
        info
    }

    #[test]
    fn workspace_sources_include_current_and_hide_external_or_unknown_pods() {
        let current = tempdir().unwrap();
        let external = tempdir().unwrap();

        let list = WorkerList::from_workspace_sources(
            SOURCE,
            vec![
                stopped_info_for_workspace("current", current.path()),
                stopped_info_for_workspace("current-orchestrator", current.path()),
                stopped_info_for_workspace("other-workspace", external.path()),
                stopped_info_with_updated_at("legacy-unknown", 10),
                corrupt_stored_info("corrupt".to_string(), "invalid metadata".to_string()),
            ],
            vec![
                live_info("current", WorkerStatus::Idle),
                live_info("current-orchestrator", WorkerStatus::Running),
                live_info("other-workspace", WorkerStatus::Idle),
                live_info("legacy-unknown", WorkerStatus::Idle),
                live_info("live-only", WorkerStatus::Idle),
            ],
            None,
            10,
            current.path(),
        );

        let names = list
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["current", "current-orchestrator"]);
        assert!(list.entries.iter().all(|entry| entry.actions.can_open));
    }

    #[test]
    fn workspace_sources_use_workspace_metadata_not_cwd_or_live_presence() {
        let current = tempdir().unwrap();
        let worktree_cwd = current.path().join(".worktree/impl");

        let list = WorkerList::from_workspace_sources(
            SOURCE,
            vec![stopped_info_for_workspace("ticket-role", current.path())],
            vec![live_info("ticket-role", WorkerStatus::Idle)],
            None,
            10,
            &worktree_cwd,
        );
        assert!(list.entries.is_empty());

        let list = WorkerList::from_workspace_sources(
            SOURCE,
            vec![stopped_info_for_workspace("ticket-role", current.path())],
            vec![live_info("ticket-role", WorkerStatus::Idle)],
            None,
            10,
            current.path(),
        );
        assert_eq!(list.entries[0].name, "ticket-role");
        assert!(list.entries[0].actions.can_open);
    }
}
