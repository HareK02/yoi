use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use client::PodClient;
use pod_registry::{LockFileGuard, default_registry_path};
use pod_store::{PodActiveSegmentRef, PodMetadata, PodMetadataStore};
use protocol::{Event, PodStatus};
use session_store::{
    FsStore, LogEntry, LoggedContentPart, LoggedItem, SegmentId, SessionId, Store,
};

#[derive(Debug, Clone)]
pub(crate) struct PodList {
    pub entries: Vec<PodListEntry>,
    pub selected_name: Option<String>,
}

impl PodList {
    pub(crate) fn from_sources(
        source: PodVisibilitySource,
        stored: Vec<StoredPodInfo>,
        live: Vec<LivePodInfo>,
        selected_name: Option<String>,
        max_entries: usize,
    ) -> Self {
        let mut entries_by_name: BTreeMap<String, PodListEntry> = BTreeMap::new();

        for live_info in live {
            let name = live_info.pod_name.clone();
            entries_by_name
                .entry(name.clone())
                .or_insert_with(|| PodListEntry::new(name, source))
                .merge_live(live_info);
        }

        for stored_info in stored {
            let name = stored_info.pod_name.clone();
            entries_by_name
                .entry(name.clone())
                .or_insert_with(|| PodListEntry::new(name, source))
                .merge_stored(stored_info);
        }

        let mut entries: Vec<PodListEntry> = entries_by_name.into_values().collect();
        for entry in &mut entries {
            entry.finalize();
        }
        entries.sort_by(|a, b| {
            b.summary
                .updated_at
                .cmp(&a.summary.updated_at)
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

    pub(crate) fn selected_index(&self) -> usize {
        self.selected_name
            .as_ref()
            .and_then(|name| self.entries.iter().position(|entry| entry.name == *name))
            .unwrap_or(0)
    }

    pub(crate) fn select_index(&mut self, index: usize) {
        self.selected_name = self.entries.get(index).map(|entry| entry.name.clone());
    }

    pub(crate) fn selected_entry(&self) -> Option<&PodListEntry> {
        let index = self.selected_index();
        self.entries.get(index)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PodVisibilitySource {
    ResumePicker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PodListSourceKind {
    RuntimeRegistry,
    StoredMetadata,
}

#[derive(Debug, Clone)]
pub(crate) struct PodListEntry {
    pub name: String,
    pub visibility: PodVisibilitySource,
    pub source_kinds: Vec<PodListSourceKind>,
    pub live: Option<LivePodInfo>,
    pub stored: Option<StoredPodInfo>,
    pub summary: PodEntrySummary,
    pub actions: PodEntryActions,
    pub diagnostics: Vec<PodEntryDiagnostic>,
}

impl PodListEntry {
    fn new(name: String, visibility: PodVisibilitySource) -> Self {
        Self {
            name,
            visibility,
            source_kinds: Vec::new(),
            live: None,
            stored: None,
            summary: PodEntrySummary::default(),
            actions: PodEntryActions::default(),
            diagnostics: Vec::new(),
        }
    }

    fn merge_live(&mut self, live: LivePodInfo) {
        if !self
            .source_kinds
            .contains(&PodListSourceKind::RuntimeRegistry)
        {
            self.source_kinds.push(PodListSourceKind::RuntimeRegistry);
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

    fn merge_stored(&mut self, stored: StoredPodInfo) {
        if !self
            .source_kinds
            .contains(&PodListSourceKind::StoredMetadata)
        {
            self.source_kinds.push(PodListSourceKind::StoredMetadata);
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
        self.diagnostics = build_diagnostics(self);
        self.actions = build_actions(self);
    }

    pub(crate) fn attach_socket_path(&self) -> Option<&Path> {
        self.live
            .as_ref()
            .filter(|live| live.reachable)
            .map(|live| live.socket_path.as_path())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct LivePodInfo {
    pub pod_name: String,
    pub socket_path: PathBuf,
    pub status: Option<PodStatus>,
    pub reachable: bool,
    pub segment_id: Option<SegmentId>,
    pub summary: PodEntrySummary,
}

#[derive(Debug, Clone)]
pub(crate) struct StoredPodInfo {
    pub pod_name: String,
    pub metadata_state: StoredMetadataState,
    pub active_session_id: Option<SessionId>,
    pub active_segment_id: Option<SegmentId>,
    pub updated_at: u64,
    pub preview: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StoredMetadataState {
    Present,
    Corrupt(String),
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PodEntrySummary {
    pub active_session_id: Option<SessionId>,
    pub active_segment_id: Option<SegmentId>,
    pub updated_at: u64,
    pub preview: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PodEntryActions {
    pub can_open: bool,
    pub can_restore: bool,
    pub can_send_now: bool,
    pub can_queue_send: bool,
    pub disabled_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PodEntryDiagnostic {
    pub kind: PodEntryDiagnosticKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PodEntryDiagnosticKind {
    StoredMetadataCorrupt,
    LiveUnreachable,
    MissingStoredMetadata,
    MissingLiveStatus,
}

pub(crate) fn read_stored_pod_infos(
    store: &FsStore,
    pod_store: &impl PodMetadataStore,
) -> Result<Vec<StoredPodInfo>, io::Error> {
    let mut records = Vec::new();
    for pod_name in pod_store.list_names().map_err(io::Error::other)? {
        let info = match pod_store.read_by_name(&pod_name) {
            Ok(Some(metadata)) => stored_info_from_metadata(store, pod_name, metadata),
            Ok(None) => corrupt_stored_info(
                pod_name,
                "metadata disappeared during discovery".to_string(),
            ),
            Err(e) => corrupt_stored_info(pod_name, e.to_string()),
        };
        records.push(info);
    }
    Ok(records)
}

pub(crate) fn read_live_pod_infos() -> Result<Vec<LivePodInfo>, io::Error> {
    let path = default_registry_path()?;
    let guard = LockFileGuard::open(&path)?;
    Ok(guard
        .data()
        .allocations
        .iter()
        .map(|allocation| LivePodInfo {
            pod_name: allocation.pod_name.clone(),
            socket_path: allocation.socket.clone(),
            status: None,
            reachable: false,
            segment_id: allocation.segment_id,
            summary: PodEntrySummary::default(),
        })
        .collect())
}

pub(crate) async fn read_reachable_live_pod_infos(
    store: &FsStore,
) -> Result<Vec<LivePodInfo>, io::Error> {
    let records = read_live_pod_infos()?;
    let mut reachable = Vec::new();
    for mut record in records {
        let Ok(status) = probe_live_status(&record.socket_path).await else {
            continue;
        };
        record.reachable = true;
        record.status = status;
        record.summary = summarize_live_pod(store, &record);
        reachable.push(record);
    }
    Ok(reachable)
}

pub(crate) fn live_socket_for_pod(pod_name: &str) -> Option<PathBuf> {
    read_live_pod_infos()
        .ok()?
        .into_iter()
        .find(|pod| pod.pod_name == pod_name)
        .map(|pod| pod.socket_path)
}

fn stored_info_from_metadata(
    store: &FsStore,
    pod_name: String,
    metadata: PodMetadata,
) -> StoredPodInfo {
    let active = metadata.active;
    let active_session_id = active.as_ref().map(|a| a.session_id);
    let active_segment_id = active.as_ref().and_then(|a| a.segment_id);
    let summary = summarize_metadata(store, active.as_ref());

    StoredPodInfo {
        pod_name,
        metadata_state: StoredMetadataState::Present,
        active_session_id,
        active_segment_id,
        updated_at: summary.updated_at,
        preview: summary.preview,
    }
}

fn corrupt_stored_info(pod_name: String, message: String) -> StoredPodInfo {
    StoredPodInfo {
        pod_name,
        metadata_state: StoredMetadataState::Corrupt(message.clone()),
        active_session_id: None,
        active_segment_id: None,
        updated_at: 0,
        preview: Some(format!("metadata: {}", trim_one_line(&message, 48))),
    }
}

const LIVE_STATUS_PROBE_TIMEOUT: Duration = Duration::from_millis(25);

async fn probe_live_status(socket_path: &Path) -> Result<Option<PodStatus>, io::Error> {
    let mut client = PodClient::connect(socket_path).await?;
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

fn status_from_event(event: &Event) -> Option<PodStatus> {
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

fn summarize_live_pod(store: &FsStore, live: &LivePodInfo) -> PodEntrySummary {
    let Some(segment_id) = live.segment_id else {
        return PodEntrySummary::default();
    };
    let session_id = store.lookup_session_of(segment_id).ok().flatten();
    let Some(session_id) = session_id else {
        return PodEntrySummary {
            active_session_id: None,
            active_segment_id: Some(segment_id),
            updated_at: 0,
            preview: None,
        };
    };
    let summary = summarize_segment(store, session_id, segment_id);
    PodEntrySummary {
        active_session_id: Some(session_id),
        active_segment_id: Some(segment_id),
        updated_at: summary.updated_at,
        preview: summary.preview,
    }
}

fn summarize_metadata(store: &FsStore, active: Option<&PodActiveSegmentRef>) -> SegmentSummary {
    let Some(active) = active else {
        return SegmentSummary {
            updated_at: 0,
            preview: None,
        };
    };
    let Some(segment_id) = active.segment_id else {
        return SegmentSummary {
            updated_at: 0,
            preview: Some("[pending segment]".to_string()),
        };
    };
    summarize_segment(store, active.session_id, segment_id)
}

fn summarize_segment(
    store: &FsStore,
    session_id: SessionId,
    segment_id: SegmentId,
) -> SegmentSummary {
    match store.read_all(session_id, segment_id) {
        Ok(entries) => SegmentSummary {
            updated_at: last_entry_ts(&entries).unwrap_or(0),
            preview: last_message_preview(&entries).or_else(|| Some("[empty]".to_string())),
        },
        Err(_) => SegmentSummary {
            updated_at: 0,
            preview: Some("[corrupt segment]".to_string()),
        },
    }
}

fn last_entry_ts(entries: &[LogEntry]) -> Option<u64> {
    entries.iter().map(log_entry_ts).max()
}

fn log_entry_ts(entry: &LogEntry) -> u64 {
    match entry {
        LogEntry::SegmentStart { ts, .. }
        | LogEntry::Invoke { ts, .. }
        | LogEntry::UserInput { ts, .. }
        | LogEntry::AssistantItem { ts, .. }
        | LogEntry::ToolResult { ts, .. }
        | LogEntry::SystemItem { ts, .. }
        | LogEntry::TurnEnd { ts, .. }
        | LogEntry::RunCompleted { ts, .. }
        | LogEntry::RunErrored { ts, .. }
        | LogEntry::ConfigChanged { ts, .. }
        | LogEntry::LlmUsage { ts, .. }
        | LogEntry::Extension { ts, .. } => *ts,
    }
}

fn last_message_preview(entries: &[LogEntry]) -> Option<String> {
    for entry in entries.iter().rev() {
        match entry {
            LogEntry::UserInput { segments, .. } => {
                let text = protocol::Segment::flatten_to_text(segments);
                if !text.is_empty() {
                    return Some(format!("user: {}", trim_one_line(&text, 60)));
                }
            }
            LogEntry::AssistantItem { item, .. } => {
                if let Some(text) = first_text_logged(item) {
                    return Some(format!("assistant: {}", trim_one_line(&text, 60)));
                }
            }
            _ => {}
        }
    }
    None
}

fn first_text_logged(item: &LoggedItem) -> Option<String> {
    match item {
        LoggedItem::Message { content, .. } => content.iter().find_map(|p| match p {
            LoggedContentPart::Text { text } => Some(text.clone()),
            _ => None,
        }),
        _ => None,
    }
}

fn build_diagnostics(entry: &PodListEntry) -> Vec<PodEntryDiagnostic> {
    let mut diagnostics = Vec::new();

    if let Some(stored) = entry.stored.as_ref() {
        if let StoredMetadataState::Corrupt(message) = &stored.metadata_state {
            diagnostics.push(PodEntryDiagnostic {
                kind: PodEntryDiagnosticKind::StoredMetadataCorrupt,
                message: format!("metadata: {}", trim_one_line(message, 80)),
            });
        }
    } else if entry.live.is_some() {
        diagnostics.push(PodEntryDiagnostic {
            kind: PodEntryDiagnosticKind::MissingStoredMetadata,
            message: "no stored pod metadata".to_string(),
        });
    }

    if let Some(live) = entry.live.as_ref() {
        if !live.reachable {
            diagnostics.push(PodEntryDiagnostic {
                kind: PodEntryDiagnosticKind::LiveUnreachable,
                message: format!("socket unreachable: {}", live.socket_path.display()),
            });
        } else if live.status.is_none() {
            diagnostics.push(PodEntryDiagnostic {
                kind: PodEntryDiagnosticKind::MissingLiveStatus,
                message: "live pod status was not reported".to_string(),
            });
        }
    }

    diagnostics
}

fn build_actions(entry: &PodListEntry) -> PodEntryActions {
    let live_reachable = entry.live.as_ref().is_some_and(|live| live.reachable);
    let stored_restorable = entry
        .stored
        .as_ref()
        .is_some_and(|stored| matches!(stored.metadata_state, StoredMetadataState::Present));
    let live_status = entry.live.as_ref().and_then(|live| live.status);

    let can_restore = stored_restorable && !live_reachable;
    let can_open = live_reachable || stored_restorable;
    let can_send_now = live_reachable && live_status == Some(PodStatus::Idle);
    let can_queue_send = live_reachable && live_status == Some(PodStatus::Running);
    let disabled_reason = if can_open {
        None
    } else if entry.live.is_some() {
        Some("live pod is unreachable".to_string())
    } else if entry.stored.is_some() {
        Some("stored pod metadata is corrupt".to_string())
    } else {
        Some("no live or stored pod state".to_string())
    };

    PodEntryActions {
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
    use llm_worker::llm_client::types::RequestConfig;
    use pod_store::FsPodStore;
    use pod_store::{PodActiveSegmentRef, PodMetadataStore};
    use session_store::{new_segment_id, new_session_id};
    use tempfile::tempdir;

    const SOURCE: PodVisibilitySource = PodVisibilitySource::ResumePicker;

    #[test]
    fn pod_list_rows_are_sorted_by_active_segment_timestamp() {
        let dir = tempdir().unwrap();
        let store = FsStore::new(dir.path()).unwrap();
        let earlier_session = new_session_id();
        let later_session = new_session_id();
        let earlier_segment = new_segment_id();
        let later_segment = new_segment_id();

        append_start(&store, earlier_session, earlier_segment, 10);
        append_user(
            &store,
            earlier_session,
            earlier_segment,
            100,
            "old pod update",
        );
        append_start(&store, later_session, later_segment, 20);
        append_user(&store, later_session, later_segment, 200, "new pod update");

        let entries = PodList::from_sources(
            SOURCE,
            vec![
                metadata_info(&store, "older", earlier_session, earlier_segment),
                metadata_info(&store, "newer", later_session, later_segment),
            ],
            vec![],
            None,
            10,
        )
        .entries;

        assert_eq!(entries[0].name, "newer");
        assert_eq!(entries[0].summary.updated_at, 200);
        assert_eq!(
            entries[0].summary.preview.as_deref(),
            Some("user: new pod update")
        );
        assert_eq!(entries[1].name, "older");
    }

    #[test]
    fn stored_only_row_can_restore_and_open_but_not_direct_send() {
        let dir = tempdir().unwrap();
        let store = FsStore::new(dir.path()).unwrap();
        let session_id = new_session_id();
        let segment_id = new_segment_id();
        append_start(&store, session_id, segment_id, 10);

        let entry = single_entry(PodList::from_sources(
            SOURCE,
            vec![metadata_info(&store, "stored", session_id, segment_id)],
            vec![],
            None,
            10,
        ));

        assert_eq!(entry.name, "stored");
        assert_eq!(entry.visibility, SOURCE);
        assert_eq!(entry.source_kinds, vec![PodListSourceKind::StoredMetadata]);
        assert!(entry.live.is_none());
        assert!(entry.stored.is_some());
        assert!(entry.actions.can_open);
        assert!(entry.actions.can_restore);
        assert!(!entry.actions.can_send_now);
        assert!(!entry.actions.can_queue_send);
    }

    #[test]
    fn live_idle_reachable_row_can_open_and_send_now() {
        let entry = single_entry(PodList::from_sources(
            SOURCE,
            vec![],
            vec![live_info("live", PodStatus::Idle)],
            None,
            10,
        ));

        assert_eq!(entry.name, "live");
        assert_eq!(entry.visibility, SOURCE);
        assert_eq!(entry.source_kinds, vec![PodListSourceKind::RuntimeRegistry]);
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
    fn live_running_reachable_row_can_open_but_not_send_now() {
        let entry = single_entry(PodList::from_sources(
            SOURCE,
            vec![],
            vec![live_info("live", PodStatus::Running)],
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
        let mut live = live_info("live", PodStatus::Idle);
        live.reachable = false;
        live.status = None;

        let entry = single_entry(PodList::from_sources(SOURCE, vec![], vec![live], None, 10));

        assert!(!entry.actions.can_open);
        assert!(!entry.actions.can_restore);
        assert!(!entry.actions.can_send_now);
        assert!(!entry.actions.can_queue_send);
        assert_eq!(
            entry.actions.disabled_reason.as_deref(),
            Some("live pod is unreachable")
        );
        assert_eq!(entry.attach_socket_path(), None);
        assert!(entry.diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == PodEntryDiagnosticKind::LiveUnreachable
                && diagnostic.message.contains("/tmp/live.sock")
        }));
    }

    #[test]
    fn status_extraction_skips_alert_before_snapshot() {
        let events = [
            Event::Alert(protocol::Alert {
                level: protocol::AlertLevel::Warn,
                source: protocol::AlertSource::Pod,
                message: "warming up".to_string(),
                timestamp_ms: 0,
            }),
            Event::Snapshot {
                entries: vec![],
                greeting: test_greeting(),
                status: PodStatus::Idle,
            },
        ];

        let status = events.iter().find_map(status_from_event);
        assert_eq!(status, Some(PodStatus::Idle));
    }

    #[test]
    fn corrupt_stored_metadata_has_diagnostic() {
        let entry = single_entry(PodList::from_sources(
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
            diagnostic.kind == PodEntryDiagnosticKind::StoredMetadataCorrupt
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
    fn selected_pod_name_is_kept_after_rebuild() {
        let first = PodList::from_sources(
            SOURCE,
            vec![],
            vec![
                live_info("alpha", PodStatus::Idle),
                live_info("beta", PodStatus::Idle),
            ],
            Some("alpha".to_string()),
            10,
        );
        assert_eq!(first.selected_entry().unwrap().name, "alpha");

        let rebuilt = PodList::from_sources(
            SOURCE,
            vec![],
            vec![
                live_info_with_updated_at("beta", PodStatus::Idle, 20),
                live_info_with_updated_at("alpha", PodStatus::Idle, 10),
            ],
            first.selected_name.clone(),
            10,
        );

        assert_eq!(rebuilt.entries[0].name, "beta");
        assert_eq!(rebuilt.selected_entry().unwrap().name, "alpha");
        assert_eq!(rebuilt.selected_index(), 1);
    }

    #[test]
    fn read_stored_pod_infos_reports_corrupt_metadata() {
        let dir = tempdir().unwrap();
        let store = FsStore::new(dir.path()).unwrap();
        let pod_store = FsPodStore::new(dir.path().join("pods")).unwrap();
        let pod_dir = dir.path().join("pods").join("broken");
        std::fs::create_dir_all(&pod_dir).unwrap();
        std::fs::write(pod_dir.join("metadata.json"), "{not-json").unwrap();

        let records = read_stored_pod_infos(&store, &pod_store).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].pod_name, "broken");
        assert!(matches!(
            records[0].metadata_state,
            StoredMetadataState::Corrupt(_)
        ));
    }

    #[test]
    fn read_stored_pod_infos_reads_metadata() {
        let dir = tempdir().unwrap();
        let store = FsStore::new(dir.path()).unwrap();
        let pod_store = FsPodStore::new(dir.path().join("pods")).unwrap();
        let session_id = new_session_id();
        let segment_id = new_segment_id();
        pod_store
            .write(&PodMetadata::new(
                "agent",
                Some(PodActiveSegmentRef::active_segment(session_id, segment_id)),
            ))
            .unwrap();

        let records = read_stored_pod_infos(&store, &pod_store).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].pod_name, "agent");
        assert_eq!(records[0].metadata_state, StoredMetadataState::Present);
    }

    fn single_entry(list: PodList) -> PodListEntry {
        assert_eq!(list.entries.len(), 1);
        list.entries.into_iter().next().unwrap()
    }

    fn metadata_info(
        store: &FsStore,
        pod_name: &str,
        session_id: SessionId,
        segment_id: SegmentId,
    ) -> StoredPodInfo {
        stored_info_from_metadata(
            store,
            pod_name.to_string(),
            PodMetadata::new(
                pod_name,
                Some(PodActiveSegmentRef::active_segment(session_id, segment_id)),
            ),
        )
    }

    fn live_info(pod_name: &str, status: PodStatus) -> LivePodInfo {
        live_info_with_updated_at(pod_name, status, 0)
    }

    fn live_info_with_updated_at(
        pod_name: &str,
        status: PodStatus,
        updated_at: u64,
    ) -> LivePodInfo {
        LivePodInfo {
            pod_name: pod_name.to_string(),
            socket_path: PathBuf::from(format!("/tmp/{pod_name}.sock")),
            status: Some(status),
            reachable: true,
            segment_id: None,
            summary: PodEntrySummary {
                active_session_id: None,
                active_segment_id: None,
                updated_at,
                preview: None,
            },
        }
    }

    fn test_greeting() -> protocol::Greeting {
        protocol::Greeting {
            pod_name: "live".to_string(),
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
}
