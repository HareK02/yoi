//! Feature exploring one immutable Session capture fixed by the Host.
//!
//! Worker-observation owns live Worker reauthorization and recapture; this Feature reads only its
//! retained snapshot. It has no Workspace client or Memory state, and follows shared Session capture
//! references, privacy, and exploration bounds. It exposes history to internal verifiers and similar
//! consumers without business-mutation capabilities.

use std::sync::Arc;

use agen::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureModule, ToolContribution,
    ToolDeclaration,
};
use crate::session_capture::{
    CapturePurpose, RawField, ReadDetail, ReadOptions, ReadSelector, ReferenceKind, SearchOptions,
    SessionCapture, SessionEntryRef, ToolPart,
};

const SHOW_OVERVIEW_DESCRIPTION: &str =
    "Show a sparse, bounded index of real user and assistant session entries.";
const SEARCH_ENTRIES_DESCRIPTION: &str = "Search or compactly list a bounded range of committed session entries. Reasoning entries are never exposed.";
const READ_ENTRY_DESCRIPTION: &str = "Read one committed session entry by stable SessionEntryRef. Compact mode is the default; full mode is a bounded rendering, not raw source. Raw mode requires raw_field and supports UTF-8 byte_offset/next_offset without omission; use capture_id to bind continuation to this exact capture.";
const DEFAULT_PAGE_LIMIT: usize = 20;
const MAX_PAGE_LIMIT: usize = 100;
const DEFAULT_READ_ITEMS: usize = 10;
const MAX_READ_ITEMS: usize = 50;
const MAX_READ_BYTES: usize = 16 * 1024;

#[derive(Clone)]
pub(crate) struct SessionExploreState {
    view: Arc<SessionCapture>,
}

impl SessionExploreState {
    pub(crate) fn new(view: SessionCapture) -> Self {
        Self {
            view: Arc::new(view),
        }
    }

    pub(crate) fn view(&self) -> &SessionCapture {
        &self.view
    }
}

#[derive(Clone)]
pub(crate) struct SessionExploreFeature {
    state: SessionExploreState,
}

impl SessionExploreFeature {
    pub(crate) fn new(state: SessionExploreState) -> Self {
        Self { state }
    }
}

impl FeatureModule for SessionExploreFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("session-explore", "Session Explore")
            .with_description(
                "Read-only exploration of one immutable host-provided session capture.",
            )
            .with_tool(ToolDeclaration::new(
                "ShowOverview",
                SHOW_OVERVIEW_DESCRIPTION,
            ))
            .with_tool(ToolDeclaration::new(
                "SearchEntries",
                SEARCH_ENTRIES_DESCRIPTION,
            ))
            .with_tool(ToolDeclaration::new("ReadEntry", READ_ENTRY_DESCRIPTION))
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        context.tools().register(ToolContribution::new(
            "ShowOverview",
            show_overview_definition(self.state.clone()),
        ))?;
        context.tools().register(ToolContribution::new(
            "SearchEntries",
            search_entries_definition(self.state.clone()),
        ))?;
        context.tools().register(ToolContribution::new(
            "ReadEntry",
            read_entry_definition(self.state.clone()),
        ))?;
        Ok(())
    }
}

fn show_overview_definition(state: SessionExploreState) -> ToolDefinition {
    Arc::new(move || {
        let schema = serde_json::to_value(schemars::schema_for!(ShowOverviewParams))
            .unwrap_or_else(|_| serde_json::json!({}));
        let meta = ToolMeta::new("ShowOverview")
            .description(SHOW_OVERVIEW_DESCRIPTION)
            .input_schema(schema);
        let tool: Arc<dyn Tool> = Arc::new(ShowOverviewTool {
            state: state.clone(),
        });
        (meta, tool)
    })
}

fn search_entries_definition(state: SessionExploreState) -> ToolDefinition {
    Arc::new(move || {
        let schema = serde_json::to_value(schemars::schema_for!(SearchEntriesParams))
            .unwrap_or_else(|_| serde_json::json!({}));
        let meta = ToolMeta::new("SearchEntries")
            .description(SEARCH_ENTRIES_DESCRIPTION)
            .input_schema(schema);
        let tool: Arc<dyn Tool> = Arc::new(SearchEntriesTool {
            state: state.clone(),
        });
        (meta, tool)
    })
}

fn read_entry_definition(state: SessionExploreState) -> ToolDefinition {
    Arc::new(move || {
        let schema = serde_json::to_value(schemars::schema_for!(ReadEntryParams))
            .unwrap_or_else(|_| serde_json::json!({}));
        let meta = ToolMeta::new("ReadEntry")
            .description(READ_ENTRY_DESCRIPTION)
            .input_schema(schema);
        let tool: Arc<dyn Tool> = Arc::new(ReadEntryTool {
            state: state.clone(),
        });
        (meta, tool)
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ShowOverviewParams {
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SearchEntriesParams {
    #[serde(default)]
    query: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    tool_part: Option<String>,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    through: Option<String>,
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadEntryParams {
    entry_ref: String,
    #[serde(default = "default_read_mode")]
    mode: String,
    #[serde(default)]
    max_items: Option<usize>,
    #[serde(default)]
    raw_field: Option<RawEntryField>,
    #[serde(default)]
    part_index: Option<usize>,
    #[serde(default)]
    byte_offset: Option<usize>,
    #[serde(default)]
    max_bytes: Option<usize>,
    #[serde(default)]
    capture_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum RawEntryField {
    MessagePart,
    PublicSystem,
    ToolArguments,
    ToolSummary,
    ToolContent,
}

impl RawEntryField {
    fn into_common(self, part_index: usize) -> RawField {
        match self {
            Self::MessagePart => RawField::MessagePart(part_index),
            Self::PublicSystem => RawField::PublicSystem,
            Self::ToolArguments => RawField::ToolArguments,
            Self::ToolSummary => RawField::ToolSummary,
            Self::ToolContent => RawField::ToolContent,
        }
    }
}

fn default_read_mode() -> String {
    "compact".to_string()
}

struct ShowOverviewTool {
    state: SessionExploreState,
}

#[async_trait]
impl Tool for ShowOverviewTool {
    async fn execute(
        &self,
        input_json: &str,
        _context: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: ShowOverviewParams = parse_input("ShowOverview", input_json)?;
        let limit = bounded_limit(params.limit, DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT);
        let overview = self.state.view().overview();
        let page = overview
            .iter()
            .skip(params.offset)
            .take(limit)
            .map(|entry| {
                serde_json::json!({
                    "entry_ref": entry.id,
                    "origin": entry.origin,
                    "entry_range": entry.entry_range,
                    "kind": entry.kind.as_str(),
                    "label": entry.label,
                    "text": entry.text,
                    "intervening_entries": entry.intervening_entries,
                })
            })
            .collect::<Vec<_>>();
        let has_more = params.offset.saturating_add(page.len()) < overview.len();
        json_output(
            format!("Showing {} session overview entrie(s).", page.len()),
            serde_json::json!({
                "entries": page,
                "offset": params.offset,
                "next_offset": has_more.then_some(params.offset + page.len()),
                "total": overview.len(),
            }),
        )
    }
}

struct SearchEntriesTool {
    state: SessionExploreState,
}

#[async_trait]
impl Tool for SearchEntriesTool {
    async fn execute(
        &self,
        input_json: &str,
        _context: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: SearchEntriesParams = parse_input("SearchEntries", input_json)?;
        let kind = params.kind.as_deref().map(parse_kind).transpose()?;
        let tool_part = params
            .tool_part
            .as_deref()
            .map(parse_tool_part)
            .transpose()?;
        let from = params.from.as_deref().map(parse_entry_ref).transpose()?;
        let through = params.through.as_deref().map(parse_entry_ref).transpose()?;
        let view = self.state.view();
        let limit = bounded_limit(params.limit, DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT);
        let hits = view
            .search_checked(&SearchOptions {
                query: params.query,
                kind,
                tool_part,
                tool_name: params.tool_name,
                limit: Some(limit),
                min_entry_index: None,
                from,
                through,
                offset: params.offset,
            })
            .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
        let entries = hits
            .iter()
            .map(|hit| {
                serde_json::json!({
                    "entry_ref": hit.id,
                    "kind": hit.kind.as_str(),
                    "tool_part": hit.tool_part.map(|part| format!("{part:?}").to_lowercase()),
                    "tool_name": hit.tool_name,
                    "label": hit.label,
                    "text": hit.summary,
                    "text_kind": "preview",
                })
            })
            .collect::<Vec<_>>();
        json_output(
            format!("Found {} session entrie(s).", entries.len()),
            serde_json::json!({
                "entries": entries,
                "offset": params.offset,
                "next_offset": (entries.len() == limit).then_some(params.offset + entries.len()),
            }),
        )
    }
}

struct ReadEntryTool {
    state: SessionExploreState,
}

#[async_trait]
impl Tool for ReadEntryTool {
    async fn execute(
        &self,
        input_json: &str,
        _context: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: ReadEntryParams = parse_input("ReadEntry", input_json)?;
        let entry_ref = parse_entry_ref(&params.entry_ref)?;
        let view = self.state.view();
        if params
            .capture_id
            .as_deref()
            .is_some_and(|id| id != view.capture_id())
        {
            return Err(ToolError::InvalidArgument(
                "capture identity does not match".into(),
            ));
        }
        if params.mode == "raw" {
            let field = params
                .raw_field
                .ok_or_else(|| ToolError::InvalidArgument("raw mode requires raw_field".into()))?;
            if params.part_index.is_some() && !matches!(field, RawEntryField::MessagePart) {
                return Err(ToolError::InvalidArgument(
                    "part_index is only valid for message_part".into(),
                ));
            }
            let reference = view
                .scoped_ref(&entry_ref)
                .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
            let raw = view
                .read_raw(
                    &reference,
                    CapturePurpose::Observation,
                    field.into_common(params.part_index.unwrap_or(0)),
                    params.byte_offset.unwrap_or(0),
                    params
                        .max_bytes
                        .unwrap_or(MAX_READ_BYTES)
                        .clamp(1, MAX_READ_BYTES),
                )
                .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
            return json_output(
                "Read raw session source bytes.".into(),
                serde_json::json!({
                    "text_kind": "raw", "raw": raw,
                }),
            );
        }
        if params.raw_field.is_some()
            || params.part_index.is_some()
            || params.byte_offset.is_some()
            || params.max_bytes.is_some()
        {
            return Err(ToolError::InvalidArgument(
                "raw fields require raw mode".into(),
            ));
        }
        let detail = match params.mode.as_str() {
            "compact" => ReadDetail::Compact,
            "full" => ReadDetail::Full,
            other => {
                return Err(ToolError::InvalidArgument(format!(
                    "invalid ReadEntry mode {other:?}; expected compact, full, or raw"
                )));
            }
        };
        let read = self.state.view().read(
            ReadSelector::Id(entry_ref.as_str()),
            ReadOptions {
                include_tools: true,
                tool_part: ToolPart::Both,
                detail,
                max_items: params
                    .max_items
                    .unwrap_or(DEFAULT_READ_ITEMS)
                    .clamp(1, MAX_READ_ITEMS),
                max_bytes: MAX_READ_BYTES,
            },
        );
        let entries = read
            .entries
            .iter()
            .map(|entry| {
                serde_json::json!({
                    "entry_ref": entry.id,
                    "origin": entry.origin,
                    "entry_range": entry.entry_range,
                    "kind": entry.kind.as_str(),
                    "tool_part": entry.tool_part.map(|part| format!("{part:?}").to_lowercase()),
                    "tool_name": entry.tool_name,
                    "label": entry.label,
                    "text": entry.text,
                })
            })
            .collect::<Vec<_>>();
        if entries.is_empty() {
            return Err(ToolError::ExecutionFailed(
                "session entry was not found in the host-provided capture".to_string(),
            ));
        }
        json_output(
            format!("Read {} session entrie(s).", entries.len()),
            serde_json::json!({
                "entries": entries,
                "truncated": read.truncated,
                "text_kind": "rendered",
            }),
        )
    }
}

fn parse_input<T: serde::de::DeserializeOwned>(
    tool_name: &str,
    input_json: &str,
) -> Result<T, ToolError> {
    serde_json::from_str(input_json)
        .map_err(|error| ToolError::InvalidArgument(format!("invalid {tool_name} input: {error}")))
}

fn parse_entry_ref(value: &str) -> Result<SessionEntryRef, ToolError> {
    SessionEntryRef::parse(value).ok_or_else(|| {
        ToolError::InvalidArgument(format!(
            "invalid SessionEntryRef {value:?}; expected E followed by a decimal source index"
        ))
    })
}

fn parse_kind(value: &str) -> Result<ReferenceKind, ToolError> {
    ReferenceKind::parse(value).ok_or_else(|| {
        ToolError::InvalidArgument(format!(
            "invalid kind {value:?}; expected user, assistant, system, or tool"
        ))
    })
}

fn parse_tool_part(value: &str) -> Result<ToolPart, ToolError> {
    ToolPart::parse(value).ok_or_else(|| {
        ToolError::InvalidArgument(format!(
            "invalid tool_part {value:?}; expected input, output, or both"
        ))
    })
}

fn bounded_limit(requested: Option<usize>, default: usize, maximum: usize) -> usize {
    requested.unwrap_or(default).clamp(1, maximum)
}

fn json_output(summary: String, value: serde_json::Value) -> Result<ToolOutput, ToolError> {
    let content = serde_json::to_string_pretty(&value)
        .map_err(|error| ToolError::ExecutionFailed(format!("serialize tool output: {error}")))?;
    Ok(ToolOutput {
        summary,
        content: Some(content),
        attachments: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use agen::Item;

    use crate::feature::{FeatureRegistryBuilder, HookRegistryBuilder};

    use super::*;

    fn state() -> SessionExploreState {
        SessionExploreState::new(SessionCapture::new(
            "session",
            "segment-1",
            vec![
                Item::user_message("first"),
                Item::reasoning("private"),
                Item::tool_call("call-1", "ExampleTool", "{}"),
                Item::assistant_message("second"),
            ],
        ))
    }

    #[test]
    fn session_explore_installs_read_only_tools_without_memory_extract() {
        let mut pending_tools = Vec::new();
        let mut hook_builder = HookRegistryBuilder::default();
        let report = FeatureRegistryBuilder::new()
            .with_module(SessionExploreFeature::new(state()))
            .install_into_pending(&mut pending_tools, &mut hook_builder);
        assert!(report.reports[0].installed);
        assert_eq!(
            report.installed_tool_names(),
            ["ShowOverview", "SearchEntries", "ReadEntry"]
        );
    }

    #[tokio::test]
    async fn overview_uses_real_entry_refs_and_rejects_unknown_fields() {
        let tool = show_overview_definition(state())().1;
        let output = tool
            .execute("{}", agen::tool::ToolExecutionContext::direct())
            .await
            .unwrap();
        let content = output.content.unwrap();
        assert!(content.contains("E00000000"));
        assert!(content.contains("E00000003"));
        assert!(content.contains("\"intervening_entries\": 1"));
        assert!(!content.contains("private"));

        let error = tool
            .execute(
                r#"{"unexpected":true}"#,
                agen::tool::ToolExecutionContext::direct(),
            )
            .await
            .unwrap_err();
        assert!(format!("{error:?}").contains("unknown field"));
    }

    #[tokio::test]
    async fn search_range_and_read_share_session_entry_refs() {
        let search = search_entries_definition(state())().1;
        let output = search
            .execute(
                r#"{"from":"E00000003","through":"E00000003"}"#,
                agen::tool::ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        let content = output.content.unwrap();
        assert!(content.contains("E00000003"));
        assert!(!content.contains("E00000000"));

        let read = read_entry_definition(state())().1;
        let output = read
            .execute(
                r#"{"entry_ref":"E00000003"}"#,
                agen::tool::ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        assert!(output.content.unwrap().contains("second"));
    }
    #[tokio::test]
    async fn search_rejects_unknown_or_reversed_capture_endpoints() {
        let search = search_entries_definition(state())().1;
        for input in [
            r#"{"from":"E99999999"}"#,
            r#"{"through":"E99999999"}"#,
            r#"{"from":"E00000003","through":"E00000000"}"#,
        ] {
            assert!(
                search
                    .execute(input, agen::tool::ToolExecutionContext::direct())
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn raw_tool_arguments_reassemble_without_preview_or_utf8_loss() {
        let arguments = "日本語\n\"\\".repeat(100);
        let state = SessionExploreState::new(SessionCapture::new(
            "session",
            "segment",
            vec![Item::tool_call("call", "Read", arguments.clone())],
        ));
        let capture_id = state.view().capture_id().to_owned();
        let tool = read_entry_definition(state)().1;
        let mut offset = 0;
        let mut assembled = String::new();
        loop {
            let input = serde_json::json!({
                "entry_ref": "E00000000", "mode": "raw", "raw_field": "tool_arguments",
                "byte_offset": offset, "max_bytes": 17, "capture_id": capture_id,
            });
            let output = tool
                .execute(
                    &input.to_string(),
                    agen::tool::ToolExecutionContext::direct(),
                )
                .await
                .unwrap();
            let value: serde_json::Value = serde_json::from_str(&output.content.unwrap()).unwrap();
            assert_eq!(value["text_kind"], "raw");
            let raw = &value["raw"];
            assert_eq!(raw["offset"], offset);
            let text = raw["text"].as_str().unwrap();
            assert!(text.len() <= 17);
            assembled.push_str(text);
            match raw["next_offset"].as_u64() {
                Some(next) => {
                    assert!(next > offset as u64);
                    offset = next as usize;
                }
                None => break,
            }
        }
        assert_eq!(assembled, arguments);
        assert!(tool.execute(r#"{"entry_ref":"E00000000","mode":"raw","raw_field":"tool_arguments","capture_id":"other"}"#,
            agen::tool::ToolExecutionContext::direct()).await.is_err());
    }
}
