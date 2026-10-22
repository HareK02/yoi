//! Exploration domain for immutable Session captures, with no Workspace/Memory mutation capability.
//!
//! A capture is an ordered projection of committed entries; exploration excludes reasoning and
//! system prompts, including Host compaction summaries and other derived context. Full inherited
//! summary input belongs to compaction, not public observation or Memory evidence. Overview,
//! search, bounded reads, and evidence conversion share `SessionEntryRef`.
//! `SessionEntryRef` is a contextual UI reference. `ScopedSessionEntryRef` additionally binds
//! the Session and exact immutable capture, and is required for portable checked operations.
//! Appends preserve existing entry identities. Sparse real user/assistant anchors and intervening
//! entry counts index long histories. Search with an empty filter also provides bounded range lists.
//!
//! Host-fixed snapshot exploration, Memory candidate evidence validation, and current Worker
//! observation share only this domain; they do not enable or modify each other's Features.
//! The provider owns authorization to recapture a current Worker. A capture itself proves neither
//! completion nor approval and grants no business-mutation authority.

use std::sync::Arc;

use crate::{
    LoggedSessionHistoryMetadata as SessionHistoryMetadata,
    LoggedSessionHistoryOrigin as HistoryOrigin,
};
mod body;
use agen::{HistoryEntry, Item, Role};
pub use body::*;
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

/// Append-stable contextual UI reference derived from a logical entry identity.
/// Portable consumers must bind it with `SessionCapture::scoped_ref` before use.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionEntryRef(String);

impl SessionEntryRef {
    pub fn from_history_entry_id(entry_id: &crate::LoggedSessionHistoryEntryId) -> Self {
        Self(format!("E{}", entry_id.0))
    }

    pub fn parse(value: &str) -> Option<Self> {
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

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionEntryRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceKind {
    User,
    Assistant,
    System,
    Tool,
}

impl ReferenceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::System => "system",
            Self::Tool => "tool",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "user" => Some(Self::User),
            "assistant" | "agent" => Some(Self::Assistant),
            "system" => Some(Self::System),
            "tool" => Some(Self::Tool),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPart {
    Input,
    Output,
    Both,
}

impl ToolPart {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "input" => Some(Self::Input),
            "output" => Some(Self::Output),
            "both" => Some(Self::Both),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
            Self::Both => "both",
        }
    }

    pub fn matches(self, actual: ToolPart) -> bool {
        matches!(self, ToolPart::Both) || self == actual
    }
}

#[derive(Debug, Clone)]
pub struct OverviewItem {
    pub id: SessionEntryRef,
    pub origin: SessionEntryProvenance,
    pub entry_range: [u64; 2],
    pub kind: ReferenceKind,
    pub label: String,
    pub text: String,
    pub intervening_entries: usize,
}

#[derive(Debug, Clone)]
pub struct ReferenceEntry {
    pub attachment_count: usize,
    use_policy: Option<protocol::SessionEntryUsePolicy>,
    pub id: SessionEntryRef,
    pub origin: SessionEntryProvenance,
    pub entry_range: [u64; 2],
    pub kind: ReferenceKind,
    pub tool_part: Option<ToolPart>,
    pub tool_name: Option<String>,
    pub label: String,
    pub summary: String,
    search_text: String,
}

#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
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
pub struct SearchHit {
    pub id: SessionEntryRef,
    pub origin: SessionEntryProvenance,
    pub kind: ReferenceKind,
    pub tool_part: Option<ToolPart>,
    pub tool_name: Option<String>,
    pub entry_range: [u64; 2],
    pub label: String,
    pub summary: String,
}

#[derive(Debug, Clone, Copy)]
pub enum ReadSelector<'a> {
    Id(&'a str),
    EntryRange([u64; 2]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadDetail {
    Compact,
    Full,
}

#[derive(Debug, Clone)]
pub struct ReadOptions {
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
pub struct ReadEntry {
    pub id: SessionEntryRef,
    pub origin: SessionEntryProvenance,
    pub kind: ReferenceKind,
    pub tool_part: Option<ToolPart>,
    pub tool_name: Option<String>,
    pub entry_range: [u64; 2],
    pub label: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct ReadResult {
    pub entries: Vec<ReadEntry>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct SessionEntryEvidence {
    pub segment_id: String,
    pub entry_ref: SessionEntryRef,
    pub origin: SessionEntryProvenance,
    /// Inclusive original-source positions in `segment_id`, not view/UI order.
    pub entry_range: [u64; 2],
    pub kind: ReferenceKind,
    pub tool_part: Option<ToolPart>,
    pub label: String,
    pub summary: String,
    pub excerpt: String,
}

#[derive(Debug, Clone)]
struct CapturedHistoryEntry {
    use_policy: Option<protocol::SessionEntryUsePolicy>,
    item: Item,
    entry_id: crate::LoggedSessionHistoryEntryId,
    origin: SessionEntryProvenance,
    dto: CaptureEntryDto,
}

#[derive(Debug, Clone)]
pub struct SessionCapture {
    session_id: String,
    segment_id: String,
    capture_id: String,
    view_entry_count: u64,
    entries: Arc<Vec<CapturedHistoryEntry>>,
    overview: Vec<OverviewItem>,
    index: Vec<ReferenceEntry>,
}

impl SessionCapture {
    pub fn from_session_snapshot(
        session_id: impl Into<String>,
        segment_id: impl Into<String>,
        snapshot: SessionSnapshot,
    ) -> Self {
        let segment_id = segment_id.into();
        let view_entry_count = snapshot.entries.len() as u64;
        let entries = snapshot
            .entries
            .into_iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                let policy = CaptureUsePolicy {
                    recording_material: false,
                    memory_evidence: entry.use_policy.is_none_or(|p| p.memory_evidence),
                    observation_readable: entry.use_policy.is_none_or(|p| p.observation_readable),
                };
                let explicit = entry.use_policy.is_some();
                let item = match entry.data {
                    SessionSnapshotEntryData::UserInput { segments } => {
                        Item::user_message(protocol::Segment::flatten_to_text(&segments))
                    }
                    SessionSnapshotEntryData::Message { role, content } => Item::Message {
                        id: None,
                        status: None,
                        role: match role {
                            SessionMessageRole::User => Role::User,
                            SessionMessageRole::Assistant => Role::Assistant,
                        },
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
                    },
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
                        ..
                    } => Item::tool_result_item(call_id, summary, content, is_error),
                    SessionSnapshotEntryData::SystemItem { content, .. } if explicit => {
                        Item::system_message(content)
                    }
                    _ => return None,
                };
                let body = CaptureBody::project(&item, &entry.provenance, explicit)?;
                if !policy.any() {
                    return None;
                }
                Some(CaptureEntryDto {
                    entry_id: entry.entry_id,
                    order: index as u64,
                    source: CaptureSource {
                        segment_id: segment_id.clone(),
                        entry_index: index as u64,
                        provenance: entry.provenance,
                        history_origin: None,
                        derived_from: entry.derived_from,
                    },
                    policy,
                    body,
                })
            })
            .collect();
        Self::from_projected(session_id.into(), segment_id, view_entry_count, entries)
    }

    pub fn new(
        session_id: impl Into<String>,
        segment_id: impl Into<String>,
        items: Vec<Item>,
    ) -> Self {
        let segment_id = segment_id.into();
        let view_entry_count = items.len() as u64;
        let entries = items
            .into_iter()
            .enumerate()
            .filter_map(|(index, item)| {
                let origin = SessionEntryProvenance::LegacyUnknown;
                let body = CaptureBody::project(&item, &origin, false)?;
                Some(CaptureEntryDto {
                    entry_id: format!("{index:08}"),
                    order: index as u64,
                    source: CaptureSource {
                        segment_id: segment_id.clone(),
                        entry_index: index as u64,
                        provenance: origin,
                        history_origin: None,
                        derived_from: Vec::new(),
                    },
                    policy: CaptureUsePolicy::PUBLIC,
                    body,
                })
            })
            .collect();
        Self::from_projected(session_id.into(), segment_id, view_entry_count, entries)
    }

    /// Project a complete Host-fixed history whose source positions start at zero.
    /// Select a suffix with `select_order_range` on the resulting value instead
    /// of passing a rebased history vector: annotations do not carry source positions.
    pub fn from_complete_history(
        session_id: impl Into<String>,
        segment_id: impl Into<String>,
        entries: Vec<HistoryEntry<SessionHistoryMetadata>>,
    ) -> Self {
        let segment_id = segment_id.into();
        let view_entry_count = entries.len() as u64;
        let entries = entries
            .into_iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                let origin = public_provenance(&entry.annotation.origin);
                let mut item = entry.item;
                let mut explicit = false;
                let policy = if let Some(input) = entry.annotation.input_metadata {
                    explicit = true;
                    if matches!(
                        item,
                        Item::Message {
                            role: Role::System,
                            ..
                        }
                    ) {
                        // Only the explicit public projection crosses this boundary.
                        let public = input.public_item()?;
                        item = Item::system_message(public.body.clone());
                    } else if input.feature_payload().is_some() || input.public_item().is_some() {
                        return None;
                    }
                    CaptureUsePolicy::from(input.usage())
                } else {
                    if matches!(entry.annotation.origin, HistoryOrigin::Provided { .. }) {
                        return None;
                    }
                    CaptureUsePolicy::PUBLIC
                };
                if !policy.any() {
                    return None;
                }
                let body = CaptureBody::project(&item, &origin, explicit)?;
                Some(CaptureEntryDto {
                    entry_id: entry.annotation.entry_id.0,
                    order: index as u64,
                    source: CaptureSource {
                        segment_id: segment_id.clone(),
                        entry_index: index as u64,
                        provenance: origin,
                        history_origin: Some(entry.annotation.origin),
                        derived_from: entry
                            .annotation
                            .derivation
                            .map(|d| d.sources.into_iter().map(|id| id.0).collect())
                            .unwrap_or_default(),
                    },
                    policy,
                    body,
                })
            })
            .collect();
        Self::from_projected(session_id.into(), segment_id, view_entry_count, entries)
    }

    fn from_projected(
        session_id: String,
        segment_id: String,
        view_entry_count: u64,
        dtos: Vec<CaptureEntryDto>,
    ) -> Self {
        let dto = CaptureDto {
            version: CAPTURE_VERSION,
            session_id: session_id.clone(),
            segment_id: segment_id.clone(),
            view_entry_count,
            entries: dtos,
        };
        let capture_id = dto.fingerprint();
        let entries: Vec<_> = dto
            .entries
            .into_iter()
            .map(|dto| CapturedHistoryEntry {
                use_policy: Some(protocol::SessionEntryUsePolicy {
                    observation_readable: dto.policy.observation_readable,
                    memory_evidence: dto.policy.memory_evidence,
                }),
                item: dto.body.to_item(),
                entry_id: crate::LoggedSessionHistoryEntryId(dto.entry_id.clone()),
                origin: dto.source.provenance.clone(),
                dto,
            })
            .collect();
        let mut overview = Vec::new();
        let mut index = Vec::new();

        for entry in &entries {
            let item = &entry.item;
            let entry_range = [entry.dto.order; 2];
            let attachment_count = match entry.dto.body {
                CaptureBody::ToolResult {
                    attachment_count, ..
                } => attachment_count,
                _ => 0,
            };
            match item {
                Item::Message { role, content, .. } => {
                    let Some(kind) = message_reference_kind(&entry.origin, role, entry.use_policy)
                    else {
                        continue;
                    };
                    let text = content
                        .iter()
                        .map(|p| p.as_text())
                        .collect::<Vec<_>>()
                        .join("");
                    let label = format!("{} message", kind.as_str());
                    let summary = truncate_chars(&text, 240);
                    let id = SessionEntryRef::from_history_entry_id(&entry.entry_id);
                    index.push(ReferenceEntry {
                        use_policy: entry.use_policy,
                        attachment_count,
                        id: id.clone(),
                        origin: entry.origin.clone(),
                        entry_range,
                        kind,
                        tool_part: None,
                        tool_name: None,
                        label: label.clone(),
                        summary: summary.clone(),
                        search_text: text.clone(),
                    });
                    if entry.use_policy.is_none_or(|p| p.observation_readable)
                        && matches!(kind, ReferenceKind::User | ReferenceKind::Assistant)
                    {
                        overview.push(OverviewItem {
                            id: id.clone(),
                            origin: entry.origin.clone(),
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
                        use_policy: entry.use_policy,
                        attachment_count,
                        id: SessionEntryRef::from_history_entry_id(&entry.entry_id),
                        origin: entry.origin.clone(),
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
                    attachments: _,
                    ..
                } => {
                    let attachment_marker = if attachment_count == 0 {
                        String::new()
                    } else {
                        format!("\n[{} image attachment(s)]", attachment_count)
                    };
                    let text = format!(
                        "{summary}\n{}{attachment_marker}",
                        content.as_deref().unwrap_or_default(),
                    );
                    index.push(ReferenceEntry {
                        use_policy: entry.use_policy,
                        attachment_count,
                        id: SessionEntryRef::from_history_entry_id(&entry.entry_id),
                        origin: entry.origin.clone(),
                        entry_range,
                        kind: ReferenceKind::Tool,
                        tool_part: Some(ToolPart::Output),
                        tool_name: None,
                        label: "tool output".to_string(),
                        summary: truncate_chars(summary, 240),
                        search_text: text,
                    });
                }
                Item::AssistantResponseBoundary { .. } | Item::Reasoning { .. } => {}
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
            session_id,
            segment_id,
            capture_id,
            view_entry_count,
            entries: Arc::new(entries),
            overview,
            index,
        }
    }

    /// Public source entries are immutable; callers retain their own business decisions.
    pub fn entries(&self) -> impl ExactSizeIterator<Item = &CaptureEntryDto> {
        self.entries.iter().map(|entry| &entry.dto)
    }

    pub fn overview(&self) -> &[OverviewItem] {
        &self.overview
    }

    /// Capture-local UI position, not the original source position used by evidence.
    pub fn view_order_for_ref(&self, reference: &SessionEntryRef) -> Option<u64> {
        self.index
            .iter()
            .find(|entry| entry.id == *reference)
            .map(|entry| entry.entry_range[0])
    }

    pub fn search(&self, options: &SearchOptions) -> Vec<SearchHit> {
        if self.validate_search(options).is_err() {
            return Vec::new();
        }
        let query = options.query.trim().to_lowercase();
        let limit = options
            .limit
            .unwrap_or(DEFAULT_SEARCH_LIMIT)
            .clamp(1, MAX_SEARCH_LIMIT);
        let tool_name = options.tool_name.as_deref();
        let min_entry_index = options
            .from
            .as_ref()
            .and_then(|reference| self.view_order_for_ref(reference))
            .unwrap_or_else(|| options.min_entry_index.unwrap_or(0));
        let max_entry_index = options
            .through
            .as_ref()
            .and_then(|reference| self.view_order_for_ref(reference))
            .unwrap_or(u64::MAX);
        let mut skipped = 0usize;
        let mut hits = Vec::new();

        for entry in &self.index {
            if entry.use_policy.is_some_and(|p| !p.observation_readable) {
                continue;
            }
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

    pub fn read(&self, selector: ReadSelector<'_>, options: ReadOptions) -> ReadResult {
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
            ReadSelector::EntryRange([start, end]) => self
                .index
                .iter()
                .filter(|entry| entry.entry_range[0] >= start && entry.entry_range[0] <= end)
                .collect(),
        };

        for entry in selected {
            if entry.use_policy.is_some_and(|p| !p.observation_readable) {
                continue;
            }
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
                .iter()
                .find(|captured| captured.dto.order == entry.entry_range[0])
                .map(|entry| &entry.item)
            else {
                continue;
            };
            let remaining = max_bytes.saturating_sub(bytes);
            if remaining == 0 {
                truncated = true;
                break;
            }
            let rendered = render_item(item, entry, options.detail, usize::MAX);
            truncated |= rendered.len() > remaining;
            let text = truncate_bytes(&rendered, remaining);
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
            if truncated {
                break;
            }
        }

        ReadResult { entries, truncated }
    }

    /// Resolves the committed tool-call input for a provider call id. This is
    /// used by explicit-memory receipts only after the containing run is durable,
    /// so a model-authored claim can cite its own committed tool invocation
    /// without inventing a SessionEntryRef.
    pub fn evidence_for_tool_call(&self, call_id: &str) -> Option<SessionEntryEvidence> {
        let captured = self.entries.iter().find(|entry| {
            matches!(
                &entry.item,
                Item::ToolCall {
                    call_id: stored_call_id,
                    ..
                } if stored_call_id == call_id
            )
        })?;
        let reference = self.index.iter().find(|entry| {
            entry.entry_range[0] == captured.dto.order
                && entry.kind == ReferenceKind::Tool
                && entry.tool_part == Some(ToolPart::Input)
        })?;
        self.evidence_for(reference.id.as_str())
    }

    pub fn evidence_for(&self, id: &str) -> Option<SessionEntryEvidence> {
        let entry = self.index.iter().find(|entry| entry.id.as_str() == id)?;
        if entry.use_policy.is_some_and(|p| !p.memory_evidence) {
            return None;
        }
        let captured = self
            .entries
            .iter()
            .find(|captured| captured.dto.order == entry.entry_range[0])?;
        let excerpt = render_item(&captured.item, entry, ReadDetail::Compact, 2 * 1024);
        Some(SessionEntryEvidence {
            segment_id: captured.dto.source.segment_id.clone(),
            entry_ref: entry.id.clone(),
            origin: entry.origin.clone(),
            entry_range: [captured.dto.source.entry_index; 2],
            kind: entry.kind,
            tool_part: entry.tool_part,
            label: entry.label.clone(),
            summary: entry.summary.clone(),
            excerpt,
        })
    }
}

fn public_provenance(origin: &HistoryOrigin) -> SessionEntryProvenance {
    match origin {
        HistoryOrigin::HumanInput { .. } | HistoryOrigin::LocalHumanInput => {
            SessionEntryProvenance::HumanInput
        }
        HistoryOrigin::WorkerInput { .. } => SessionEntryProvenance::WorkerInput,
        HistoryOrigin::FlowInstruction { .. } => SessionEntryProvenance::FlowInstruction,
        HistoryOrigin::BackendInstruction { .. } | HistoryOrigin::Provided { .. } => {
            SessionEntryProvenance::BackendInstruction
        }
        HistoryOrigin::ModelOutput { .. } => SessionEntryProvenance::ModelOutput,
        HistoryOrigin::ToolOutput { .. } => SessionEntryProvenance::ToolOutput,
        HistoryOrigin::CompactionSummary
        | HistoryOrigin::DerivedSummary
        | HistoryOrigin::JournalContext => SessionEntryProvenance::DerivedSummary,
        HistoryOrigin::LegacyUnknown => SessionEntryProvenance::LegacyUnknown,
    }
}

fn message_reference_kind(
    origin: &SessionEntryProvenance,
    provider_role: &Role,
    use_policy: Option<protocol::SessionEntryUsePolicy>,
) -> Option<ReferenceKind> {
    if use_policy.is_some() {
        return match provider_role {
            Role::System => Some(ReferenceKind::System),
            Role::User => Some(ReferenceKind::User),
            Role::Assistant => Some(ReferenceKind::Assistant),
        };
    }
    match origin {
        SessionEntryProvenance::HumanInput | SessionEntryProvenance::WorkerInput => {
            Some(ReferenceKind::User)
        }
        SessionEntryProvenance::ModelOutput => Some(ReferenceKind::Assistant),
        SessionEntryProvenance::ToolOutput => Some(ReferenceKind::Tool),
        SessionEntryProvenance::LegacyUnknown => match provider_role {
            Role::User => Some(ReferenceKind::User),
            Role::Assistant => Some(ReferenceKind::Assistant),
            Role::System => None,
        },
        // Flow/backend/system content remains out of the observation surface
        // even when represented with a provider user/system role.
        SessionEntryProvenance::FlowInstruction
        | SessionEntryProvenance::BackendInstruction
        | SessionEntryProvenance::DerivedSummary => None,
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
            attachments: _,
            is_error,
            ..
        } => {
            let attachment_line = if entry.attachment_count == 0 {
                String::new()
            } else {
                format!("\nattachments: {} image(s)", entry.attachment_count)
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
        Item::AssistantResponseBoundary { .. } => {
            format!("[{} Assistant response boundary omitted]", entry.id)
        }
        Item::Reasoning { .. } => format!("[{} Reasoning omitted]", entry.id),
    };
    truncate_bytes(&text, max_bytes)
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

fn truncate_bytes(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let marker = "… [truncated]";
    let mut end = max_bytes.saturating_sub(marker.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = text[..end].to_owned();
    if max_bytes >= marker.len() {
        out.push_str(marker);
    }
    out
}
