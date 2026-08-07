use std::sync::Arc;

use async_trait::async_trait;
use llm_engine::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureModule, ToolContribution,
    ToolDeclaration,
};
use crate::session_capture::{
    ReadDetail, ReadOptions, ReadSelector, ReferenceKind, SearchOptions, SessionCapture,
    SessionEntryRef, ToolPart,
};

const SHOW_OVERVIEW_DESCRIPTION: &str =
    "Show a sparse, bounded index of real user and assistant session entries.";
const SEARCH_ENTRIES_DESCRIPTION: &str = "Search or compactly list a bounded range of committed session entries. Reasoning entries are never exposed.";
const READ_ENTRY_DESCRIPTION: &str = "Read one committed session entry by stable SessionEntryRef. Compact mode is the default; full mode includes bounded tool input or output.";
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
        _context: llm_engine::tool::ToolExecutionContext,
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
        _context: llm_engine::tool::ToolExecutionContext,
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
        if let (Some(from), Some(through)) = (&from, &through) {
            if from.source_index() > through.source_index() {
                return Err(ToolError::InvalidArgument(
                    "SearchEntries from must not be after through".to_string(),
                ));
            }
        }
        let limit = bounded_limit(params.limit, DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT);
        let hits = self.state.view().search(&SearchOptions {
            query: params.query,
            kind,
            tool_part,
            tool_name: params.tool_name,
            limit: Some(limit),
            min_entry_index: None,
            from,
            through,
            offset: params.offset,
        });
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
        _context: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: ReadEntryParams = parse_input("ReadEntry", input_json)?;
        let entry_ref = parse_entry_ref(&params.entry_ref)?;
        let detail = match params.mode.as_str() {
            "compact" => ReadDetail::Compact,
            "full" => ReadDetail::Full,
            other => {
                return Err(ToolError::InvalidArgument(format!(
                    "invalid ReadEntry mode {other:?}; expected compact or full"
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
            "invalid kind {value:?}; expected user, assistant, or tool"
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
    })
}

#[cfg(test)]
mod tests {
    use llm_engine::Item;

    use crate::feature::{FeatureRegistryBuilder, HookRegistryBuilder};

    use super::*;

    fn state() -> SessionExploreState {
        SessionExploreState::new(SessionCapture::new(
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
            .execute("{}", llm_engine::tool::ToolExecutionContext::direct())
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
                llm_engine::tool::ToolExecutionContext::direct(),
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
                llm_engine::tool::ToolExecutionContext::direct(),
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
                llm_engine::tool::ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        assert!(output.content.unwrap().contains("second"));
    }
}
