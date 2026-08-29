//! Workspace- and Memory-independent exploration of an immutable ordered session capture.
//!
//! Hosts construct a capture from committed session items. The capture excludes reasoning,
//! assigns append-stable `SessionEntryRef` values, and provides sparse overview, bounded
//! range/search, read, and generic evidence projections without granting mutation authority.

use std::sync::Arc;

use crate::session_history::{SessionHistoryMetadata, WorkerHistoryProvenance};
use agen::{HistoryEntry, Item, Role};
use protocol::{
    SessionContentPart, SessionEntryProvenance, SessionMessageRole, SessionSnapshot,
    SessionSnapshotEntryData,
};
use serde::{Deserialize, Serialize};

const DEFAULT_SEARCH_LIMIT: usize = 20;
const MAX_SEARCH_LIMIT: usize = 50;
const DEFAULT_READ_MAX_ITEMS: usize = 40;
const MAX_READ_MAX_ITEMS: usize = 80;
const DEFAULT_READ_MAX_BYTES: usize = 32 * 1024;
const OVERVIEW_ANCHOR_STRIDE: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct SessionEntryRef(String);

impl SessionEntryRef {
    pub(crate) fn from_history_entry_id(entry_id: &crate::SessionHistoryEntryId) -> Self {
        Self(format!("E{}", entry_id.0))
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        let suffix = value.strip_prefix('E')?;
        if suffix.is_empty()
            || suffix.len() > 64
            || !suffix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return None;
        }
        Some(Self(value.to_string()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn source_index(&self) -> Option<u64> {
        self.0.strip_prefix('E')?.parse().ok()
    }
}

impl std::fmt::Display for SessionEntryRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReferenceKind {
    User,
    Assistant,
    Tool,
}

impl ReferenceKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "user" => Some(Self::User),
            "assistant" | "agent" => Some(Self::Assistant),
            "tool" => Some(Self::Tool),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolPart {
    Input,
    Output,
    Both,
}

impl ToolPart {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "input" => Some(Self::Input),
            "output" => Some(Self::Output),
            "both" => Some(Self::Both),
            _ => None,
        }
    }

    fn matches(self, actual: ToolPart) -> bool {
        matches!(self, ToolPart::Both) || self == actual
    }
}

#[derive(Debug, Clone)]
pub(crate) struct OverviewItem {
    pub id: SessionEntryRef,
    pub origin: WorkerHistoryProvenance,
    pub entry_range: [u64; 2],
    pub kind: ReferenceKind,
    pub label: String,
    pub text: String,
    pub intervening_entries: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct ReferenceEntry {
    pub id: SessionEntryRef,
    pub origin: WorkerHistoryProvenance,
    pub entry_range: [u64; 2],
    pub kind: ReferenceKind,
    pub tool_part: Option<ToolPart>,
    pub tool_name: Option<String>,
    pub label: String,
    pub summary: String,
    search_text: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SearchOptions {
    pub query: String,
    pub kind: Option<ReferenceKind>,
    pub tool_part: Option<ToolPart>,
    pub tool_name: Option<String>,
    pub limit: Option<usize>,
    pub min_entry_index: Option<u64>,
    pub from: Option<SessionEntryRef>,
    pub through: Option<SessionEntryRef>,
    pub offset: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct SearchHit {
    pub id: SessionEntryRef,
    pub origin: WorkerHistoryProvenance,
    pub kind: ReferenceKind,
    pub tool_part: Option<ToolPart>,
    pub tool_name: Option<String>,
    pub entry_range: [u64; 2],
    pub label: String,
    pub summary: String,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ReadSelector<'a> {
    Id(&'a str),
    #[cfg(test)]
    EntryRange([u64; 2]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadDetail {
    Compact,
    Full,
}

#[derive(Debug, Clone)]
pub(crate) struct ReadOptions {
    pub include_tools: bool,
    pub tool_part: ToolPart,
    pub detail: ReadDetail,
    pub max_items: usize,
    pub max_bytes: usize,
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            include_tools: true,
            tool_part: ToolPart::Both,
            detail: ReadDetail::Compact,
            max_items: DEFAULT_READ_MAX_ITEMS,
            max_bytes: DEFAULT_READ_MAX_BYTES,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ReadEntry {
    pub id: SessionEntryRef,
    pub origin: WorkerHistoryProvenance,
    pub kind: ReferenceKind,
    pub tool_part: Option<ToolPart>,
    pub tool_name: Option<String>,
    pub entry_range: [u64; 2],
    pub label: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ReadResult {
    pub entries: Vec<ReadEntry>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct SessionEntryEvidence {
    pub segment_id: String,
    pub entry_ref: SessionEntryRef,
    pub origin: WorkerHistoryProvenance,
    pub entry_range: [u64; 2],
    pub kind: ReferenceKind,
    pub tool_part: Option<ToolPart>,
    pub label: String,
    pub summary: String,
    pub excerpt: String,
}

#[derive(Debug, Clone)]
pub(crate) struct SessionCapture {
    segment_id: String,
    entries: Arc<Vec<HistoryEntry<SessionHistoryMetadata>>>,
    overview: Vec<OverviewItem>,
    index: Vec<ReferenceEntry>,
}

impl SessionCapture {
    pub(crate) fn from_session_snapshot(
        segment_id: impl Into<String>,
        snapshot: SessionSnapshot,
    ) -> Self {
        let entries = snapshot
            .entries
            .into_iter()
            .filter_map(|entry| {
                let item = match entry.data {
                    SessionSnapshotEntryData::UserInput { segments } => {
                        Item::user_message(protocol::Segment::flatten_to_text(&segments))
                    }
                    SessionSnapshotEntryData::Message { role, content } => {
                        let role = match role {
                            SessionMessageRole::User => Role::User,
                            SessionMessageRole::Assistant => Role::Assistant,
                        };
                        Item::Message {
                            id: None,
                            role,
                            content: content
                                .into_iter()
                                .map(|part| match part {
                                    SessionContentPart::Text { text } => {
                                        agen::ContentPart::Text { text }
                                    }
                                    SessionContentPart::Refusal { refusal } => {
                                        agen::ContentPart::Refusal { refusal }
                                    }
                                })
                                .collect(),
                            status: None,
                        }
                    }
                    SessionSnapshotEntryData::ToolCall {
                        call_id,
                        name,
                        arguments,
                    } => Item::tool_call(call_id, name, arguments),
                    SessionSnapshotEntryData::ToolResult {
                        call_id,
                        summary,
                        content,
                        is_error,
                        attachments: _,
                    } => Item::tool_result_item(call_id, summary, content, is_error),
                    // Observation deliberately excludes system items and
                    // controller errors from model-visible session evidence.
                    SessionSnapshotEntryData::SystemItem { .. }
                    | SessionSnapshotEntryData::RunError { .. } => return None,
                };
                Some(HistoryEntry::new(
                    item,
                    public_snapshot_metadata(entry.entry_id, entry.provenance),
                ))
            })
            .collect();
        Self::from_history_entries(segment_id, entries)
    }

    pub(crate) fn new(segment_id: impl Into<String>, items: Vec<Item>) -> Self {
        let entries = items
            .into_iter()
            .enumerate()
            .map(|(index, item)| {
                let mut metadata = SessionHistoryMetadata::legacy_unknown();
                metadata.entry_id =
                    session_store::LoggedSessionHistoryEntryId(format!("{index:08}"));
                HistoryEntry::new(item, metadata)
            })
            .collect();
        Self::from_history_entries(segment_id, entries)
    }

    pub(crate) fn from_history_entries(
        segment_id: impl Into<String>,
        entries: Vec<HistoryEntry<SessionHistoryMetadata>>,
    ) -> Self {
        let segment_id = segment_id.into();
        let entries = Arc::new(entries);
        let mut overview = Vec::new();
        let mut index = Vec::new();

        for (idx, entry) in entries.iter().enumerate() {
            let item = &entry.item;
            let entry_range = [idx as u64, idx as u64];
            match item {
                Item::Message { role, content, .. } => {
                    let Some(kind) = message_reference_kind(&entry.annotation.origin, role) else {
                        continue;
                    };
                    let text = content
                        .iter()
                        .map(|p| p.as_text())
                        .collect::<Vec<_>>()
                        .join("");
                    let label = format!("{} message", kind.as_str());
                    let summary = truncate_chars(&text, 240);
                    let id = SessionEntryRef::from_history_entry_id(&entry.annotation.entry_id);
                    index.push(ReferenceEntry {
                        id: id.clone(),
                        origin: entry.annotation.origin.clone(),
                        entry_range,
                        kind,
                        tool_part: None,
                        tool_name: None,
                        label: label.clone(),
                        summary: summary.clone(),
                        search_text: text.clone(),
                    });
                    if matches!(kind, ReferenceKind::User | ReferenceKind::Assistant) {
                        overview.push(OverviewItem {
                            id: id.clone(),
                            origin: entry.annotation.origin.clone(),
                            entry_range,
                            kind,
                            label,
                            text,
                            intervening_entries: 0,
                        });
                    }
                }
                Item::ToolCall {
                    name, arguments, ..
                } => {
                    let text = format!("{name}\n{arguments}");
                    index.push(ReferenceEntry {
                        id: SessionEntryRef::from_history_entry_id(&entry.annotation.entry_id),
                        origin: entry.annotation.origin.clone(),
                        entry_range,
                        kind: ReferenceKind::Tool,
                        tool_part: Some(ToolPart::Input),
                        tool_name: Some(name.clone()),
                        label: format!("tool input: {name}"),
                        summary: format!("Tool call {name}"),
                        search_text: text,
                    });
                }
                Item::ToolResult {
                    summary,
                    content,
                    attachments,
                    ..
                } => {
                    let attachment_marker = if attachments.is_empty() {
                        String::new()
                    } else {
                        format!("\n[{} image attachment(s)]", attachments.len())
                    };
                    let text = format!(
                        "{summary}\n{}{attachment_marker}",
                        content.as_deref().unwrap_or_default(),
                    );
                    index.push(ReferenceEntry {
                        id: SessionEntryRef::from_history_entry_id(&entry.annotation.entry_id),
                        origin: entry.annotation.origin.clone(),
                        entry_range,
                        kind: ReferenceKind::Tool,
                        tool_part: Some(ToolPart::Output),
                        tool_name: None,
                        label: "tool output".to_string(),
                        summary: truncate_chars(summary, 240),
                        search_text: text,
                    });
                }
                Item::Reasoning { .. } => {}
            }
        }

        if overview.len() > 2 {
            let last = overview.len() - 1;
            overview = overview
                .into_iter()
                .enumerate()
                .filter_map(|(index, entry)| {
                    (index == 0 || index == last || index % OVERVIEW_ANCHOR_STRIDE == 0)
                        .then_some(entry)
                })
                .collect();
        }

        for overview_index in 0..overview.len().saturating_sub(1) {
            let current_entry = overview[overview_index].entry_range[0];
            let next_entry = overview[overview_index + 1].entry_range[0];
            overview[overview_index].intervening_entries = index
                .iter()
                .filter(|entry| {
                    let entry_index = entry.entry_range[0];
                    entry_index > current_entry && entry_index < next_entry
                })
                .count();
        }

        Self {
            segment_id,
            entries,
            overview,
            index,
        }
    }

    pub(crate) fn overview(&self) -> &[OverviewItem] {
        &self.overview
    }

    pub(crate) fn source_index_for_ref(&self, reference: &SessionEntryRef) -> Option<u64> {
        self.index
            .iter()
            .find(|entry| entry.id == *reference)
            .map(|entry| entry.entry_range[0])
            .or_else(|| reference.source_index())
    }

    pub(crate) fn search(&self, options: &SearchOptions) -> Vec<SearchHit> {
        let query = options.query.trim().to_lowercase();
        let limit = options
            .limit
            .unwrap_or(DEFAULT_SEARCH_LIMIT)
            .clamp(1, MAX_SEARCH_LIMIT);
        let tool_name = options.tool_name.as_deref();
        let min_entry_index = options
            .from
            .as_ref()
            .and_then(|reference| self.source_index_for_ref(reference))
            .unwrap_or_else(|| options.min_entry_index.unwrap_or(0));
        let max_entry_index = options
            .through
            .as_ref()
            .and_then(|reference| self.source_index_for_ref(reference))
            .unwrap_or(u64::MAX);
        let mut skipped = 0usize;
        let mut hits = Vec::new();

        for entry in &self.index {
            if entry.entry_range[0] < min_entry_index || entry.entry_range[0] > max_entry_index {
                continue;
            }
            if let Some(kind) = options.kind {
                if entry.kind != kind {
                    continue;
                }
            }
            if let Some(part) = options.tool_part {
                if entry.kind != ReferenceKind::Tool {
                    continue;
                }
                let Some(actual) = entry.tool_part else {
                    continue;
                };
                if !part.matches(actual) {
                    continue;
                }
            }
            if let Some(name) = tool_name {
                if entry.tool_name.as_deref() != Some(name) {
                    continue;
                }
            }
            if !query.is_empty() && !entry.search_text.to_lowercase().contains(&query) {
                continue;
            }
            if skipped < options.offset {
                skipped += 1;
                continue;
            }
            hits.push(SearchHit {
                id: entry.id.clone(),
                origin: entry.origin.clone(),
                kind: entry.kind,
                tool_part: entry.tool_part,
                tool_name: entry.tool_name.clone(),
                entry_range: entry.entry_range,
                label: entry.label.clone(),
                summary: entry.summary.clone(),
            });
            if hits.len() >= limit {
                break;
            }
        }

        hits
    }

    pub(crate) fn read(&self, selector: ReadSelector<'_>, options: ReadOptions) -> ReadResult {
        let max_items = options.max_items.clamp(1, MAX_READ_MAX_ITEMS);
        let max_bytes = options.max_bytes.max(1);
        let mut entries = Vec::new();
        let mut bytes = 0usize;
        let mut truncated = false;

        let selected: Vec<&ReferenceEntry> = match selector {
            ReadSelector::Id(id) => self
                .index
                .iter()
                .filter(|entry| entry.id.as_str() == id)
                .collect(),
            #[cfg(test)]
            ReadSelector::EntryRange([start, end]) => self
                .index
                .iter()
                .filter(|entry| entry.entry_range[0] >= start && entry.entry_range[0] <= end)
                .collect(),
        };

        for entry in selected {
            if entries.len() >= max_items {
                truncated = true;
                break;
            }
            if entry.kind == ReferenceKind::Tool {
                if !options.include_tools {
                    continue;
                }
                if let Some(actual) = entry.tool_part {
                    if !options.tool_part.matches(actual) {
                        continue;
                    }
                }
            }
            let Some(item) = self
                .entries
                .get(entry.entry_range[0] as usize)
                .map(|entry| &entry.item)
            else {
                continue;
            };
            let text = render_item(item, entry, options.detail, max_bytes.saturating_sub(bytes));
            bytes = bytes.saturating_add(text.len());
            entries.push(ReadEntry {
                id: entry.id.clone(),
                origin: entry.origin.clone(),
                kind: entry.kind,
                tool_part: entry.tool_part,
                tool_name: entry.tool_name.clone(),
                entry_range: entry.entry_range,
                label: entry.label.clone(),
                text,
            });
            if bytes >= max_bytes {
                truncated = true;
                break;
            }
        }

        ReadResult { entries, truncated }
    }

    pub(crate) fn evidence_for(&self, id: &str) -> Option<SessionEntryEvidence> {
        let entry = self.index.iter().find(|entry| entry.id.as_str() == id)?;
        let excerpt = self
            .read(
                ReadSelector::Id(id),
                ReadOptions {
                    include_tools: true,
                    tool_part: ToolPart::Both,
                    detail: ReadDetail::Compact,
                    max_items: 1,
                    max_bytes: 2 * 1024,
                },
            )
            .entries
            .first()
            .map(|entry| entry.text.clone())
            .unwrap_or_else(|| entry.summary.clone());
        Some(SessionEntryEvidence {
            segment_id: self.segment_id.clone(),
            entry_ref: entry.id.clone(),
            origin: entry.origin.clone(),
            entry_range: entry.entry_range,
            kind: entry.kind,
            tool_part: entry.tool_part,
            label: entry.label.clone(),
            summary: entry.summary.clone(),
            excerpt,
        })
    }
}

fn public_snapshot_metadata(
    entry_id: String,
    provenance: SessionEntryProvenance,
) -> SessionHistoryMetadata {
    let worker = session_store::LoggedWorkerSubject {
        workspace_id: None,
        runtime_id: None,
        worker_id: "public-session-snapshot".to_owned(),
    };
    let origin = match provenance {
        SessionEntryProvenance::HumanInput => WorkerHistoryProvenance::HumanInput {
            account_id: "public-session-snapshot".to_owned(),
        },
        SessionEntryProvenance::WorkerInput => WorkerHistoryProvenance::WorkerInput {
            actor: worker.clone(),
        },
        SessionEntryProvenance::FlowInstruction => WorkerHistoryProvenance::FlowInstruction {
            selector: "public-session-snapshot".to_owned(),
            definition_id: "public-session-snapshot".to_owned(),
            definition_revision: 0,
            instance_id: "public-session-snapshot".to_owned(),
            state_id: "public-session-snapshot".to_owned(),
        },
        SessionEntryProvenance::BackendInstruction => {
            WorkerHistoryProvenance::BackendInstruction { operation_id: None }
        }
        SessionEntryProvenance::ModelOutput => WorkerHistoryProvenance::ModelOutput {
            worker: worker.clone(),
        },
        SessionEntryProvenance::ToolOutput => WorkerHistoryProvenance::ToolOutput { worker },
        SessionEntryProvenance::DerivedSummary => WorkerHistoryProvenance::DerivedSummary,
        SessionEntryProvenance::LegacyUnknown => WorkerHistoryProvenance::LegacyUnknown,
    };
    SessionHistoryMetadata {
        entry_id: session_store::LoggedSessionHistoryEntryId(entry_id),
        origin,
        derivation: None,
    }
}

fn message_reference_kind(
    origin: &WorkerHistoryProvenance,
    provider_role: &Role,
) -> Option<ReferenceKind> {
    match origin {
        WorkerHistoryProvenance::HumanInput { .. }
        | WorkerHistoryProvenance::WorkerInput { .. } => Some(ReferenceKind::User),
        WorkerHistoryProvenance::ModelOutput { .. } => Some(ReferenceKind::Assistant),
        WorkerHistoryProvenance::ToolOutput { .. } => Some(ReferenceKind::Tool),
        WorkerHistoryProvenance::LegacyUnknown => match provider_role {
            Role::User => Some(ReferenceKind::User),
            Role::Assistant => Some(ReferenceKind::Assistant),
            Role::System => None,
        },
        // Flow/backend/system content remains out of the observation surface
        // even when represented with a provider user/system role.
        WorkerHistoryProvenance::FlowInstruction { .. }
        | WorkerHistoryProvenance::BackendInstruction { .. }
        | WorkerHistoryProvenance::DerivedSummary => None,
    }
}

fn render_item(
    item: &Item,
    entry: &ReferenceEntry,
    detail: ReadDetail,
    max_bytes: usize,
) -> String {
    let text = match item {
        Item::Message { role, content, .. } => {
            let text = content
                .iter()
                .map(|p| p.as_text())
                .collect::<Vec<_>>()
                .join("");
            format!("[{} {:?}] {text}", entry.id, role)
        }
        Item::ToolCall {
            name, arguments, ..
        } => match detail {
            ReadDetail::Compact => format!("[{} ToolInput {name}] (arguments omitted)", entry.id),
            ReadDetail::Full => format!("[{} ToolInput {name}]\narguments: {arguments}", entry.id),
        },
        Item::ToolResult {
            summary,
            content,
            attachments,
            is_error,
            ..
        } => {
            let attachment_line = if attachments.is_empty() {
                String::new()
            } else {
                format!("\nattachments: {} image(s)", attachments.len())
            };
            match detail {
                ReadDetail::Compact => format!(
                    "[{} ToolOutput{}]\nsummary: {summary}\ncontent: (omitted){attachment_line}",
                    entry.id,
                    if *is_error { " error" } else { "" },
                ),
                ReadDetail::Full => format!(
                    "[{} ToolOutput{}]\nsummary: {summary}\ncontent: {}{attachment_line}",
                    entry.id,
                    if *is_error { " error" } else { "" },
                    content.as_deref().unwrap_or_default(),
                ),
            }
        }
        Item::Reasoning { .. } => format!("[{} Reasoning omitted]", entry.id),
    };
    truncate_chars(&text, max_bytes)
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return "… [truncated]".to_string();
    }
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out = text.chars().take(max_chars).collect::<String>();
    out.push_str("… [truncated]");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_user_role_is_excluded_while_explicit_human_origin_remains_evidence() {
        let entries = vec![
            crate::session_history::history_entry(
                Item::user_message("trusted flow instruction"),
                WorkerHistoryProvenance::FlowInstruction {
                    selector: "builtin:coder-review".into(),
                    definition_id: "coder-review".into(),
                    definition_revision: 3,
                    instance_id: "instance".into(),
                    state_id: "implement".into(),
                },
            ),
            crate::session_history::history_entry(
                Item::user_message("remember my preference"),
                WorkerHistoryProvenance::HumanInput {
                    account_id: "account-1".into(),
                },
            ),
        ];
        let capture = SessionCapture::from_history_entries("segment", entries);
        let overview = capture.overview();
        assert_eq!(overview.len(), 1);
        assert!(matches!(
            overview[0].origin,
            WorkerHistoryProvenance::HumanInput { .. }
        ));
        let evidence = capture.evidence_for(overview[0].id.as_str()).unwrap();
        assert!(evidence.excerpt.ends_with("remember my preference"));
        assert!(matches!(
            evidence.origin,
            WorkerHistoryProvenance::HumanInput { .. }
        ));
    }

    #[test]
    fn stable_logical_ref_survives_retention_and_restore_projection() {
        let retained = crate::session_history::history_entry(
            Item::assistant_message("retained"),
            WorkerHistoryProvenance::ModelOutput {
                worker: crate::session_history::worker_subject(Default::default()),
            },
        );
        let expected_ref = SessionEntryRef::from_history_entry_id(&retained.annotation.entry_id);
        let before = SessionCapture::from_history_entries("old", vec![retained.clone()]);
        let after = SessionCapture::from_history_entries("new", vec![retained]);
        assert_eq!(before.overview()[0].id, expected_ref);
        assert_eq!(after.overview()[0].id, expected_ref);
        assert_eq!(
            after.evidence_for(expected_ref.as_str()).unwrap().entry_ref,
            expected_ref
        );
    }

    #[test]
    fn overview_contains_user_and_assistant_only() {
        let view = SessionCapture::new(
            "segment-1",
            vec![
                Item::system_message("sys"),
                Item::user_message("hello"),
                Item::assistant_message("progress"),
                Item::tool_call("c1", "Read", "{\"file\":\"x\"}"),
                Item::tool_result("c1", "read ok"),
            ],
        );

        let overview = view.overview();
        assert_eq!(overview.len(), 2);
        assert_eq!(overview[0].kind, ReferenceKind::User);
        assert_eq!(overview[1].kind, ReferenceKind::Assistant);
        assert!(overview[1].text.contains("progress"));
    }

    #[test]
    fn search_filters_tool_input_and_output() {
        let view = SessionCapture::new(
            "segment-1",
            vec![
                Item::tool_call("c1", "Read", "{\"file\":\"Cargo.toml\"}"),
                Item::tool_result_with_content("c1", "read ok", "package metadata"),
            ],
        );

        let input_hits = view.search(&SearchOptions {
            query: "Cargo.toml".into(),
            kind: Some(ReferenceKind::Tool),
            tool_part: Some(ToolPart::Input),
            tool_name: Some("Read".into()),
            limit: None,
            min_entry_index: None,
            from: None,
            through: None,
            offset: 0,
        });
        assert_eq!(input_hits.len(), 1);
        assert_eq!(input_hits[0].tool_part, Some(ToolPart::Input));

        let output_hits = view.search(&SearchOptions {
            query: "package metadata".into(),
            kind: Some(ReferenceKind::Tool),
            tool_part: Some(ToolPart::Output),
            tool_name: None,
            limit: None,
            min_entry_index: None,
            from: None,
            through: None,
            offset: 0,
        });
        assert_eq!(output_hits.len(), 1);
        assert_eq!(output_hits[0].tool_part, Some(ToolPart::Output));
    }

    #[test]
    fn read_by_entry_range_is_bounded_and_can_skip_tools() {
        let view = SessionCapture::new(
            "segment-1",
            vec![
                Item::user_message("one"),
                Item::tool_call("c1", "Read", "{}"),
                Item::tool_result_with_content("c1", "ok", "large-content".repeat(100)),
                Item::assistant_message("two"),
            ],
        );

        let result = view.read(
            ReadSelector::EntryRange([0, 3]),
            ReadOptions {
                include_tools: false,
                detail: ReadDetail::Compact,
                max_items: 10,
                max_bytes: 1_000,
                ..ReadOptions::default()
            },
        );

        assert_eq!(result.entries.len(), 2);
        assert!(
            result
                .entries
                .iter()
                .all(|entry| entry.kind != ReferenceKind::Tool)
        );
    }

    #[test]
    fn tool_image_projection_exposes_only_bounded_metadata() {
        let view = SessionCapture::new(
            "segment-1",
            vec![Item::tool_result_item_with_attachments(
                "c1",
                "attached",
                None,
                false,
                vec![agen::tool::Attachment::Image(
                    agen::tool::ImageAttachment::new("image/png", b"private-image-body".to_vec()),
                )],
            )],
        );

        let result = view.read(
            ReadSelector::Id("E00000000"),
            ReadOptions {
                include_tools: true,
                detail: ReadDetail::Full,
                ..ReadOptions::default()
            },
        );
        assert_eq!(result.entries.len(), 1);
        assert!(result.entries[0].text.contains("attachments: 1 image(s)"));
        assert!(!result.entries[0].text.contains("private-image-body"));
        assert!(!result.entries[0].text.contains("cHJpdmF0ZS"));
    }

    #[test]
    fn system_prompt_and_reasoning_are_excluded_from_every_projection() {
        let view = SessionCapture::new(
            "segment-1",
            vec![
                Item::system_message("raw secret system prompt"),
                Item::reasoning("private chain of thought"),
                Item::user_message("visible user entry"),
            ],
        );
        let hits = view.search(&SearchOptions {
            query: String::new(),
            kind: None,
            tool_part: None,
            tool_name: None,
            limit: None,
            min_entry_index: None,
            from: None,
            through: None,
            offset: 0,
        });
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id.as_str(), "E00000002");
        assert!(!hits[0].summary.contains("secret"));
        assert!(!hits[0].summary.contains("chain of thought"));
        assert!(
            view.read(ReadSelector::Id("E00000000"), ReadOptions::default())
                .entries
                .is_empty()
        );
        assert!(view.evidence_for("E00000000").is_none());
        assert_eq!(view.overview().len(), 1);
        assert_eq!(view.overview()[0].id.as_str(), "E00000002");
    }

    #[test]
    fn overview_is_sparse_and_reports_intervening_non_reasoning_entries() {
        let items = (0..20)
            .map(|index| Item::user_message(format!("message-{index}")))
            .collect::<Vec<_>>();
        let view = SessionCapture::new("segment-1", items);
        let refs = view
            .overview()
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            refs,
            vec!["E00000000", "E00000008", "E00000016", "E00000019"]
        );
        assert_eq!(view.overview()[0].intervening_entries, 7);
        assert_eq!(view.overview()[1].intervening_entries, 7);
        assert_eq!(view.overview()[2].intervening_entries, 2);
    }

    #[test]
    fn append_preserves_existing_session_entry_refs() {
        let first = SessionCapture::new(
            "segment-1",
            vec![
                Item::user_message("first"),
                Item::assistant_message("second"),
            ],
        );
        let appended = SessionCapture::new(
            "segment-1",
            vec![
                Item::user_message("first"),
                Item::assistant_message("second"),
                Item::user_message("third"),
            ],
        );
        let first_refs = first
            .search(&SearchOptions {
                query: String::new(),
                kind: None,
                tool_part: None,
                tool_name: None,
                limit: None,
                min_entry_index: None,
                from: None,
                through: None,
                offset: 0,
            })
            .into_iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>();
        let appended_refs = appended
            .search(&SearchOptions {
                query: String::new(),
                kind: None,
                tool_part: None,
                tool_name: None,
                limit: None,
                min_entry_index: None,
                from: None,
                through: None,
                offset: 0,
            })
            .into_iter()
            .take(2)
            .map(|entry| entry.id)
            .collect::<Vec<_>>();
        assert_eq!(first_refs, appended_refs);
    }

    #[test]
    fn evidence_projection_uses_entry_range_and_session_entry_ref() {
        let view = SessionCapture::new("segment-1", vec![Item::user_message("hello")]);
        let source = view.evidence_for("E00000000").unwrap();
        assert_eq!(source.segment_id, "segment-1");
        assert_eq!(source.entry_range, [0, 0]);
        assert_eq!(source.entry_ref.as_str(), "E00000000");
    }
}
