//! Immutable reference view over a session history slice.
//!
//! This module is shared substrate for internal workers that need to inspect a
//! bounded, host-created view of session history without reading the live
//! foreground Worker state directly.

use std::sync::Arc;

use llm_engine::{Item, Role};
use memory::extract::StagingEvidence;
use memory::schema::{EvidenceKind, SourceEvidenceRef};

const DEFAULT_SEARCH_LIMIT: usize = 20;
const MAX_SEARCH_LIMIT: usize = 50;
const DEFAULT_READ_MAX_ITEMS: usize = 40;
const MAX_READ_MAX_ITEMS: usize = 80;
const DEFAULT_READ_MAX_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReferenceKind {
    User,
    Assistant,
    System,
    Tool,
}

impl ReferenceKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::System => "system",
            Self::Tool => "tool",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "user" => Some(Self::User),
            "assistant" | "agent" => Some(Self::Assistant),
            "system" => Some(Self::System),
            "tool" => Some(Self::Tool),
            _ => None,
        }
    }

    fn evidence_kind(self) -> EvidenceKind {
        EvidenceKind::new(EvidenceKind::MESSAGE)
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
    pub id: String,
    pub entry_range: [u64; 2],
    pub kind: ReferenceKind,
    pub label: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ReferenceEntry {
    pub id: String,
    pub entry_range: [u64; 2],
    pub kind: ReferenceKind,
    pub tool_part: Option<ToolPart>,
    pub tool_name: Option<String>,
    pub label: String,
    pub summary: String,
    search_text: String,
}

impl ReferenceEntry {
    fn evidence_kind(&self) -> EvidenceKind {
        match (self.kind, self.tool_part) {
            (ReferenceKind::Tool, Some(ToolPart::Input)) => {
                EvidenceKind::new(EvidenceKind::TOOL_CALL)
            }
            (ReferenceKind::Tool, Some(ToolPart::Output | ToolPart::Both) | None) => {
                EvidenceKind::new(EvidenceKind::TOOL_RESULT)
            }
            _ => self.kind.evidence_kind(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SearchOptions {
    pub query: String,
    pub kind: Option<ReferenceKind>,
    pub tool_part: Option<ToolPart>,
    pub tool_name: Option<String>,
    pub limit: Option<usize>,
    pub min_entry_index: Option<u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct SearchHit {
    pub id: String,
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
    pub id: String,
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
pub(crate) struct SessionReferenceView {
    segment_id: String,
    items: Arc<Vec<Item>>,
    overview: Vec<OverviewItem>,
    index: Vec<ReferenceEntry>,
}

impl SessionReferenceView {
    pub(crate) fn new(segment_id: impl Into<String>, items: Vec<Item>) -> Self {
        let segment_id = segment_id.into();
        let items = Arc::new(items);
        let mut overview = Vec::new();
        let mut index = Vec::new();

        for (idx, item) in items.iter().enumerate() {
            let entry_range = [idx as u64, idx as u64];
            match item {
                Item::Message { role, content, .. } => {
                    let kind = match role {
                        Role::User => ReferenceKind::User,
                        Role::Assistant => ReferenceKind::Assistant,
                        Role::System => ReferenceKind::System,
                    };
                    let text = content
                        .iter()
                        .map(|p| p.as_text())
                        .collect::<Vec<_>>()
                        .join("");
                    let label = format!("{} message", kind.as_str());
                    let summary = truncate_chars(&text, 240);
                    let id = format!("M{idx:04}");
                    index.push(ReferenceEntry {
                        id: id.clone(),
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
                            id: format!("O{:04}", overview.len()),
                            entry_range,
                            kind,
                            label,
                            text,
                        });
                    }
                }
                Item::ToolCall {
                    name, arguments, ..
                } => {
                    let text = format!("{name}\n{arguments}");
                    index.push(ReferenceEntry {
                        id: format!("T{idx:04}i"),
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
                    summary, content, ..
                } => {
                    let text = format!("{summary}\n{}", content.as_deref().unwrap_or_default());
                    index.push(ReferenceEntry {
                        id: format!("T{idx:04}o"),
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

        Self {
            segment_id,
            items,
            overview,
            index,
        }
    }

    pub(crate) fn overview(&self) -> &[OverviewItem] {
        &self.overview
    }

    pub(crate) fn search(&self, options: &SearchOptions) -> Vec<SearchHit> {
        let query = options.query.trim().to_lowercase();
        let limit = options
            .limit
            .unwrap_or(DEFAULT_SEARCH_LIMIT)
            .clamp(1, MAX_SEARCH_LIMIT);
        let tool_name = options.tool_name.as_deref();
        let min_entry_index = options.min_entry_index.unwrap_or(0);
        let mut hits = Vec::new();

        for entry in &self.index {
            if entry.entry_range[0] < min_entry_index {
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
            hits.push(SearchHit {
                id: entry.id.clone(),
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
            ReadSelector::Id(id) => self.index.iter().filter(|entry| entry.id == id).collect(),
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
            let Some(item) = self.items.get(entry.entry_range[0] as usize) else {
                continue;
            };
            let text = render_item(item, entry, options.detail, max_bytes.saturating_sub(bytes));
            bytes = bytes.saturating_add(text.len());
            entries.push(ReadEntry {
                id: entry.id.clone(),
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

    pub(crate) fn source_ref_for(&self, id: &str) -> Option<SourceEvidenceRef> {
        let entry = self.index.iter().find(|entry| entry.id == id)?;
        Some(SourceEvidenceRef {
            segment_id: Some(self.segment_id.clone()),
            entry_range: Some(entry.entry_range),
            evidence_id: Some(entry.id.clone()),
            evidence_kind: Some(entry.evidence_kind()),
            label: Some(entry.label.clone()),
            summary: Some(entry.summary.clone()),
            ..Default::default()
        })
    }

    pub(crate) fn staging_evidence_for(&self, id: &str) -> Option<StagingEvidence> {
        let entry = self.index.iter().find(|entry| entry.id == id)?;
        let read = self.read(
            ReadSelector::Id(id),
            ReadOptions {
                include_tools: true,
                tool_part: ToolPart::Both,
                detail: ReadDetail::Compact,
                max_items: 1,
                max_bytes: 2 * 1024,
            },
        );
        let excerpt = read
            .entries
            .first()
            .map(|entry| entry.text.clone())
            .unwrap_or_else(|| entry.summary.clone());
        Some(StagingEvidence {
            id: entry.id.clone(),
            kind: entry.evidence_kind(),
            entry_range: Some(entry.entry_range),
            excerpt: Some(excerpt),
            summary: Some(entry.summary.clone()),
        })
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
            is_error,
            ..
        } => match detail {
            ReadDetail::Compact => format!(
                "[{} ToolOutput{}]\nsummary: {summary}\ncontent: (omitted)",
                entry.id,
                if *is_error { " error" } else { "" }
            ),
            ReadDetail::Full => format!(
                "[{} ToolOutput{}]\nsummary: {summary}\ncontent: {}",
                entry.id,
                if *is_error { " error" } else { "" },
                content.as_deref().unwrap_or_default()
            ),
        },
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
    fn overview_contains_user_and_assistant_only() {
        let view = SessionReferenceView::new(
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
        let view = SessionReferenceView::new(
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
        });
        assert_eq!(output_hits.len(), 1);
        assert_eq!(output_hits[0].tool_part, Some(ToolPart::Output));
    }

    #[test]
    fn read_by_entry_range_is_bounded_and_can_skip_tools() {
        let view = SessionReferenceView::new(
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
    fn source_ref_uses_entry_range_and_evidence_id() {
        let view = SessionReferenceView::new("segment-1", vec![Item::user_message("hello")]);
        let source = view.source_ref_for("M0000").unwrap();
        assert_eq!(source.segment_id.as_deref(), Some("segment-1"));
        assert_eq!(source.entry_range, Some([0, 0]));
        assert_eq!(source.evidence_id.as_deref(), Some("M0000"));
    }
}
