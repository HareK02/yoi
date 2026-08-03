//! Compact worker state and the four tools that drive it.
//!
//! The compact worker is a disposable `Engine` instance spun up by
//! [`Worker::compact`]. It receives the history to summarise plus a list of
//! default reference files (from the session-lifetime `Tracker`) and runs
//! a tool-driven LLM loop. The tools here let it:
//!
//! - `read_file` — inspect referenced files (reuses `tools::read_tool`)
//! - `mark_read_required(path, offset?, limit?)` — nominate a file whose
//!   contents should be injected into the compacted context as an
//!   auto-read system message
//! - `add_reference(path)` — nominate a file the next session should
//!   know about by name only (contents not included)
//! - `write_summary(text)` — deliver (or overwrite) the structured summary
//!
//! Everything the worker decides ends up in [`CompactWorkerContext`],
//! which `Worker::compact` drains after the loop and turns into the
//! compacted session's opening system messages.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use llm_engine::Item;
use llm_engine::interceptor::{Interceptor, PreRequestAction, PreToolAction, ToolCallInfo};
use llm_engine::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput, ToolResult};
use serde::Deserialize;
#[cfg(test)]
use workdir::LocalWorkdirSession;
use workdir::{ReadRequest, WorkdirPath, WorkdirSessionHandle};

use crate::compact::usage_tracker::UsageTracker;
use crate::fs_view::ReadRequirement;
#[cfg(test)]
use crate::fs_view::slice_lines;
use crate::session_reference::{
    ReadDetail, ReadOptions, ReadSelector, SearchOptions, SessionReferenceView, ToolPart,
};

/// Aggregated output of a compact worker run.
#[derive(Debug, Default, Clone)]
pub(crate) struct CompactWorkerContext {
    pub read_required: Vec<ReadRequirement>,
    pub references: Vec<PathBuf>,
    pub summary: Option<String>,
    /// Tokens already consumed by `mark_read_required` calls.
    pub auto_read_consumed: u64,
    /// Aggregate cap. `0` treats the budget as disabled.
    pub auto_read_budget: u64,
}

impl CompactWorkerContext {
    pub(crate) fn with_budget(auto_read_budget: u64) -> Self {
        Self {
            auto_read_budget,
            ..Self::default()
        }
    }

    fn remaining_budget(&self) -> u64 {
        self.auto_read_budget
            .saturating_sub(self.auto_read_consumed)
    }
}

/// Input to `mark_read_required`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct MarkParams {
    /// Absolute path to the file.
    pub file_path: PathBuf,
    /// 0-based line offset.
    #[serde(default)]
    pub offset: Option<usize>,
    /// Maximum number of lines to inject.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Input to `add_reference`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ReferenceParams {
    /// Absolute path to the file.
    pub file_path: PathBuf,
}

/// Input to `write_summary`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SummaryParams {
    /// Full structured summary text (overwrites any previous call).
    pub text: String,
}

/// Input to `search_session_log`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SearchSessionParams {
    /// Case-insensitive substring to search in compact-target history.
    pub query: String,
    /// 0-based item offset to start searching from.
    #[serde(default)]
    pub offset: Option<usize>,
    /// Maximum number of hits to return.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Input to `read_session_items`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ReadSessionParams {
    /// 0-based compact-target history item offset.
    pub offset: usize,
    /// Maximum number of items to return.
    pub limit: usize,
    /// `compact` omits tool arguments/full results; `full` includes message text and tool result content.
    #[serde(default = "default_session_read_mode")]
    pub mode: String,
}

fn default_session_read_mode() -> String {
    "compact".to_string()
}

const SESSION_TOOL_MAX_OUTPUT_TOKENS: u64 = 12_000;
const SESSION_SEARCH_MAX_RESULTS: usize = 50;
const SESSION_READ_MAX_ITEMS: usize = 80;

const MARK_DESCRIPTION: &str = "Inject a file's contents into the compacted context so the \
next session starts with it already read. Use this for files the next task needs in full. \
Optionally specify `offset` (0-based line) and `limit` (line count) to inject only a slice. \
Counts against `auto_read_budget`; overflow returns an error and the mark is not recorded. \
Paths must be absolute.";

const REFERENCE_DESCRIPTION: &str = "Record a file path as a named reference in the compacted \
context without injecting its contents. Use for files that are contextually relevant but \
whose current content the next session can fetch on demand.";

const SUMMARY_DESCRIPTION: &str = "Provide the final structured summary text. Subsequent calls \
replace the previous content; only the last call is used. Must be called before the compact run \
ends or compaction fails.";

const SEARCH_SESSION_DESCRIPTION: &str = "Search the compact-target session history by \
case-insensitive substring. Returns item indexes and compact snippets. Use this when the initial \
overview is not enough to identify which part of the session matters. Results are bounded; narrow \
the query if important details are omitted.";

const READ_SESSION_DESCRIPTION: &str = "Read a bounded range of compact-target session history \
items by 0-based index. mode='compact' omits tool arguments, full tool results, and reasoning \
bodies; mode='full' includes message text and tool result content but still remains bounded. Use \
this to verify details before writing the summary.";

struct SessionLogToolState {
    items: Arc<Vec<Item>>,
    view: SessionReferenceView,
}

struct SearchSessionLogTool {
    state: Arc<SessionLogToolState>,
}

#[async_trait]
impl Tool for SearchSessionLogTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: SearchSessionParams = serde_json::from_str(input_json).map_err(|e| {
            ToolError::InvalidArgument(format!("invalid search_session_log input: {e}"))
        })?;
        let query = params.query.trim().to_lowercase();
        if query.is_empty() {
            return Err(ToolError::InvalidArgument(
                "search_session_log query must not be empty".to_string(),
            ));
        }
        let offset = params.offset.unwrap_or(0);
        let limit = params
            .limit
            .unwrap_or(20)
            .clamp(1, SESSION_SEARCH_MAX_RESULTS);
        let hits = self.state.view.search(&SearchOptions {
            query: params.query.clone(),
            kind: None,
            tool_part: None,
            tool_name: None,
            limit: Some(limit),
            min_entry_index: Some(offset as u64),
        });
        let blocks = hits
            .iter()
            .map(|hit| {
                let part = hit
                    .tool_part
                    .map(|part| format!(" {part:?}"))
                    .unwrap_or_default();
                let tool = hit
                    .tool_name
                    .as_ref()
                    .map(|name| format!(" {name}"))
                    .unwrap_or_default();
                format!(
                    "[{} {}{}{} {:?}] {}\n{}",
                    hit.id,
                    hit.kind.as_str(),
                    part,
                    tool,
                    hit.entry_range,
                    hit.label,
                    hit.summary
                )
            })
            .collect::<Vec<_>>();
        let mut content = blocks.join("\n\n");
        let truncated = truncate_to_token_budget(&mut content, SESSION_TOOL_MAX_OUTPUT_TOKENS);
        let summary = if hits.is_empty() {
            format!("No session log hits for {query:?} from item offset {offset}.")
        } else if truncated {
            format!(
                "Found {} session log hit(s) for {query:?}; output truncated. Narrow the query.",
                hits.len()
            )
        } else {
            format!("Found {} session log hit(s) for {query:?}.", hits.len())
        };
        Ok(ToolOutput {
            summary,
            content: (!content.is_empty()).then_some(content),
        })
    }
}

struct ReadSessionItemsTool {
    state: Arc<SessionLogToolState>,
}

#[async_trait]
impl Tool for ReadSessionItemsTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: ReadSessionParams = serde_json::from_str(input_json).map_err(|e| {
            ToolError::InvalidArgument(format!("invalid read_session_items input: {e}"))
        })?;
        let mode = SessionReadMode::parse(&params.mode)?;
        let offset = params.offset.min(self.state.items.len());
        let limit = params.limit.clamp(1, SESSION_READ_MAX_ITEMS);
        let end = offset.saturating_add(limit).min(self.state.items.len());
        let detail = match mode {
            SessionReadMode::Compact => ReadDetail::Compact,
            SessionReadMode::Full => ReadDetail::Full,
        };
        let read = if offset >= end {
            crate::session_reference::ReadResult {
                entries: Vec::new(),
                truncated: false,
            }
        } else {
            self.state.view.read(
                ReadSelector::EntryRange([offset as u64, end.saturating_sub(1) as u64]),
                ReadOptions {
                    include_tools: true,
                    tool_part: ToolPart::Both,
                    detail,
                    max_items: limit,
                    max_bytes: 48 * 1024,
                },
            )
        };
        let mut content = read
            .entries
            .iter()
            .map(|entry| entry.text.clone())
            .collect::<Vec<_>>()
            .join("\n\n");
        let token_truncated =
            truncate_to_token_budget(&mut content, SESSION_TOOL_MAX_OUTPUT_TOKENS);
        let truncated = read.truncated || token_truncated;
        let summary = if truncated {
            format!(
                "Read session items {offset}..{end} in {mode:?} mode; output truncated. Narrow the range."
            )
        } else {
            format!("Read session items {offset}..{end} in {mode:?} mode.")
        };
        Ok(ToolOutput {
            summary,
            content: (!content.is_empty()).then_some(content),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionReadMode {
    Compact,
    Full,
}

impl SessionReadMode {
    fn parse(value: &str) -> Result<Self, ToolError> {
        match value {
            "compact" => Ok(Self::Compact),
            "full" => Ok(Self::Full),
            other => Err(ToolError::InvalidArgument(format!(
                "invalid read_session_items mode {other:?}; expected 'compact' or 'full'"
            ))),
        }
    }
}

fn truncate_to_token_budget(text: &mut String, max_tokens: u64) -> bool {
    let max_bytes = max_tokens.saturating_mul(4) as usize;
    if text.len() <= max_bytes {
        return false;
    }
    let mut cut = 0;
    for (idx, _) in text.char_indices() {
        if idx > max_bytes {
            break;
        }
        cut = idx;
    }
    text.truncate(cut);
    text.push_str("\n… [session tool output truncated]");
    true
}

struct MarkReadRequiredTool {
    session: WorkdirSessionHandle,
    ctx: Arc<Mutex<CompactWorkerContext>>,
}

#[async_trait]
impl Tool for MarkReadRequiredTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: MarkParams = serde_json::from_str(input_json).map_err(|e| {
            ToolError::InvalidArgument(format!("invalid mark_read_required input: {e}"))
        })?;

        // Read through the shared WorkdirSession so scope and I/O errors surface the
        // same way the regular `read_file` tool does.
        let path = WorkdirPath::new(params.file_path.to_string_lossy())
            .map_err(|error| ToolError::InvalidArgument(error.to_string()))?;
        let result = self
            .session
            .read(ReadRequest {
                path,
                offset: params.offset.unwrap_or(0),
                limit: params.limit.unwrap_or(usize::MAX),
                max_bytes: 4 * 1024 * 1024,
            })
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("read failed: {e}")))?;
        let text = String::from_utf8_lossy(&result.bytes);
        let slice = text.as_ref();
        let estimated_tokens = estimate_tokens(slice.len());

        let mut guard = self.ctx.lock().expect("compact worker context poisoned");
        let budget = guard.auto_read_budget;
        let would_consume = guard.auto_read_consumed.saturating_add(estimated_tokens);
        if budget > 0 && would_consume > budget {
            return Err(ToolError::ExecutionFailed(format!(
                "auto-read budget exhausted ({budget} tokens). Remove an existing mark or use \
                 add_reference instead."
            )));
        }
        guard.read_required.push(ReadRequirement {
            path: params.file_path.clone(),
            offset: params.offset,
            limit: params.limit,
        });
        guard.auto_read_consumed = would_consume;
        let remaining = guard.remaining_budget();
        drop(guard);

        let mut summary = format!(
            "Marked {} for auto-read (≈{estimated_tokens} tokens). \
             Budget: {remaining}/{budget} tokens remaining.",
            params.file_path.display()
        );
        if budget > 0 && remaining * 2 <= budget {
            summary.push_str(
                "\nNote: auto-read budget is at least half consumed. \
                 Consider calling write_summary and finishing up soon.",
            );
        }
        Ok(ToolOutput {
            summary,
            content: None,
        })
    }
}

struct AddReferenceTool {
    ctx: Arc<Mutex<CompactWorkerContext>>,
}

#[async_trait]
impl Tool for AddReferenceTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: ReferenceParams = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid add_reference input: {e}")))?;
        let mut guard = self.ctx.lock().expect("compact worker context poisoned");
        if !guard
            .references
            .iter()
            .any(|p| p.as_path() == params.file_path.as_path())
        {
            guard.references.push(params.file_path.clone());
        }
        Ok(ToolOutput {
            summary: format!("Added reference {}", params.file_path.display()),
            content: None,
        })
    }
}

struct WriteSummaryTool {
    ctx: Arc<Mutex<CompactWorkerContext>>,
}

#[async_trait]
impl Tool for WriteSummaryTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: SummaryParams = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid write_summary input: {e}")))?;
        let mut guard = self.ctx.lock().expect("compact worker context poisoned");
        let overwritten = guard.summary.is_some();
        guard.summary = Some(params.text);
        drop(guard);
        let note = if overwritten {
            "Summary replaced."
        } else {
            "Summary recorded."
        };
        Ok(ToolOutput {
            summary: note.to_string(),
            content: None,
        })
    }
}

pub(crate) fn mark_read_required_tool(
    session: WorkdirSessionHandle,
    ctx: Arc<Mutex<CompactWorkerContext>>,
) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(MarkParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("mark_read_required")
            .description(MARK_DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(MarkReadRequiredTool {
            session: session.clone(),
            ctx: ctx.clone(),
        });
        (meta, tool)
    })
}

pub(crate) fn add_reference_tool(ctx: Arc<Mutex<CompactWorkerContext>>) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(ReferenceParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("add_reference")
            .description(REFERENCE_DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(AddReferenceTool { ctx: ctx.clone() });
        (meta, tool)
    })
}

pub(crate) fn write_summary_tool(ctx: Arc<Mutex<CompactWorkerContext>>) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(SummaryParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("write_summary")
            .description(SUMMARY_DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(WriteSummaryTool { ctx: ctx.clone() });
        (meta, tool)
    })
}

pub(crate) fn search_session_log_tool(items: Arc<Vec<Item>>) -> ToolDefinition {
    let view = SessionReferenceView::new("compact-target", (*items).clone());
    let state = Arc::new(SessionLogToolState { items, view });
    Arc::new(move || {
        let schema = schemars::schema_for!(SearchSessionParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("search_session_log")
            .description(SEARCH_SESSION_DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(SearchSessionLogTool {
            state: state.clone(),
        });
        (meta, tool)
    })
}

pub(crate) fn read_session_items_tool(items: Arc<Vec<Item>>) -> ToolDefinition {
    let view = SessionReferenceView::new("compact-target", (*items).clone());
    let state = Arc::new(SessionLogToolState { items, view });
    Arc::new(move || {
        let schema = schemars::schema_for!(ReadSessionParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("read_session_items")
            .description(READ_SESSION_DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(ReadSessionItemsTool {
            state: state.clone(),
        });
        (meta, tool)
    })
}

/// Interceptor that monitors compact-worker context occupancy.
///
/// `max_input_tokens` remains the hard circuit breaker. Before that point,
/// the interceptor can persist a system warning into worker history telling
/// the model to stop broad exploration and call `write_summary`, and can block
/// additional exploratory tool calls once the final reserve is reached.
pub(crate) struct CompactWorkerInterceptor {
    pub usage_tracker: Arc<UsageTracker>,
    pub max_input_tokens: u64,
    pub finish_warning_remaining_tokens: u64,
    pub final_reserve_tokens: u64,
    pub on_warning: Option<Arc<dyn Fn(String) + Send + Sync>>,
    warning_sent: AtomicBool,
    last_remaining_tokens: AtomicU64,
}

impl CompactWorkerInterceptor {
    pub(crate) fn new(
        usage_tracker: Arc<UsageTracker>,
        max_input_tokens: u64,
        finish_warning_remaining_tokens: u64,
        final_reserve_tokens: u64,
        on_warning: Option<Arc<dyn Fn(String) + Send + Sync>>,
    ) -> Self {
        Self {
            usage_tracker,
            max_input_tokens,
            finish_warning_remaining_tokens,
            final_reserve_tokens,
            on_warning,
            warning_sent: AtomicBool::new(false),
            last_remaining_tokens: AtomicU64::new(max_input_tokens),
        }
    }

    fn maybe_emit_warning(&self, remaining: u64) -> Option<Item> {
        let warning_threshold = self.finish_warning_remaining_tokens;
        let reserve_threshold = self.final_reserve_tokens;
        let should_warn = (warning_threshold > 0 && remaining <= warning_threshold)
            || (reserve_threshold > 0 && remaining <= reserve_threshold);
        if !should_warn || self.warning_sent.swap(true, Ordering::AcqRel) {
            return None;
        }

        let message = format!(
            "compact worker context budget is low ({remaining}/{} tokens remaining). \
             Stop broad exploration now, read only if absolutely necessary, then call \
             `write_summary` with the final structured summary.",
            self.max_input_tokens
        );
        if let Some(cb) = self.on_warning.as_ref() {
            cb(message.clone());
        }
        Some(Item::system_message(format!(
            "[Compact worker budget warning]\n\n{message}"
        )))
    }
}

#[async_trait]
impl Interceptor for CompactWorkerInterceptor {
    async fn pre_llm_request(&self, context: &mut Vec<Item>) -> PreRequestAction {
        let records = self.usage_tracker.records();
        let estimate = llm_engine::token_counter::total_tokens(context, &records);
        if estimate.tokens > self.max_input_tokens {
            return PreRequestAction::Cancel(format!(
                "compact worker input occupancy exceeded {} tokens",
                self.max_input_tokens
            ));
        }

        let remaining = self.max_input_tokens.saturating_sub(estimate.tokens);
        self.last_remaining_tokens
            .store(remaining, Ordering::Release);
        if let Some(item) = self.maybe_emit_warning(remaining) {
            self.usage_tracker.note_request(context.len() + 1);
            return PreRequestAction::ContinueWith(vec![item]);
        }

        self.usage_tracker.note_request(context.len());
        PreRequestAction::Continue
    }

    async fn pre_tool_call(&self, info: &mut ToolCallInfo) -> PreToolAction {
        if self.final_reserve_tokens == 0 || info.call.name == "write_summary" {
            return PreToolAction::Continue;
        }
        let remaining = self.last_remaining_tokens.load(Ordering::Acquire);
        if remaining > self.final_reserve_tokens {
            return PreToolAction::Continue;
        }
        PreToolAction::SyntheticResult(ToolResult::error(
            info.call.id.clone(),
            "compact worker final reserve reached; do not perform more exploratory tool reads. Call `write_summary` now.",
        ))
    }
}

/// Crude bytes→tokens estimate; good enough for budget accounting.
fn estimate_tokens(bytes: usize) -> u64 {
    (bytes as u64).div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use manifest::Scope;

    fn make_fs(tmp: &std::path::Path) -> WorkdirSessionHandle {
        let scope = Scope::writable(tmp.to_path_buf()).unwrap();
        Arc::new(LocalWorkdirSession::new(scope, tmp.to_path_buf()))
    }

    fn make_usage(input: u64) -> llm_engine::timeline::event::UsageEvent {
        llm_engine::timeline::event::UsageEvent {
            input_tokens: Some(input),
            output_tokens: Some(0),
            total_tokens: Some(input),
            cache_read_input_tokens: None,
            cache_creation_input_tokens: None,
        }
    }

    #[tokio::test]
    async fn compact_worker_interceptor_uses_occupancy_not_cumulative_usage() {
        let tracker = Arc::new(UsageTracker::new());
        let interceptor = CompactWorkerInterceptor::new(tracker.clone(), 150, 0, 0, None);
        let mut context = vec![Item::user_message("hello")];

        assert!(matches!(
            interceptor.pre_llm_request(&mut context).await,
            PreRequestAction::Continue
        ));
        tracker.record_usage(&make_usage(100));

        assert!(matches!(
            interceptor.pre_llm_request(&mut context).await,
            PreRequestAction::Continue
        ));
        tracker.record_usage(&make_usage(100));

        // Two 100-token requests would exceed a cumulative 150-token cap, but
        // current occupancy is still the latest 100-token measurement.
        assert!(matches!(
            interceptor.pre_llm_request(&mut context).await,
            PreRequestAction::Continue
        ));
    }

    #[tokio::test]
    async fn compact_worker_interceptor_warns_before_hard_cap() {
        let tracker = Arc::new(UsageTracker::new());
        let warnings = Arc::new(Mutex::new(Vec::new()));
        let captured = warnings.clone();
        let interceptor = CompactWorkerInterceptor::new(
            tracker.clone(),
            150,
            60,
            20,
            Some(Arc::new(move |message| {
                captured.lock().unwrap().push(message);
            })),
        );
        let mut context = vec![Item::user_message("hello")];

        assert!(matches!(
            interceptor.pre_llm_request(&mut context).await,
            PreRequestAction::Continue
        ));
        tracker.record_usage(&make_usage(100));

        assert!(matches!(
            interceptor.pre_llm_request(&mut context).await,
            PreRequestAction::ContinueWith(items)
                if items.len() == 1 && items[0].as_text().unwrap_or_default().contains("write_summary")
        ));
        assert_eq!(warnings.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn compact_worker_interceptor_cancels_when_occupancy_exceeds_cap() {
        let tracker = Arc::new(UsageTracker::new());
        let interceptor = CompactWorkerInterceptor::new(tracker.clone(), 99, 0, 0, None);
        let mut context = vec![Item::user_message("hello")];

        assert!(matches!(
            interceptor.pre_llm_request(&mut context).await,
            PreRequestAction::Continue
        ));
        tracker.record_usage(&make_usage(100));

        assert!(matches!(
            interceptor.pre_llm_request(&mut context).await,
            PreRequestAction::Cancel(message) if message.contains("occupancy")
        ));
    }

    #[tokio::test]
    async fn mark_read_required_records_and_deducts_budget() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("hello.txt");
        std::fs::write(&path, "hello world\n").unwrap();

        let ctx = Arc::new(Mutex::new(CompactWorkerContext::with_budget(1_000)));
        let tool: Arc<dyn Tool> = Arc::new(MarkReadRequiredTool {
            session: make_fs(tmp.path()),
            ctx: ctx.clone(),
        });
        let input = serde_json::json!({ "file_path": path.file_name().unwrap().to_str().unwrap() })
            .to_string();
        let out = tool.execute(&input, Default::default()).await.unwrap();

        assert!(out.summary.starts_with("Marked"));
        let guard = ctx.lock().unwrap();
        assert_eq!(guard.read_required.len(), 1);
        assert!(guard.auto_read_consumed > 0);
        assert!(guard.auto_read_consumed <= 1_000);
    }

    #[tokio::test]
    async fn mark_read_required_rejects_over_budget() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("big.txt");
        std::fs::write(&path, "x".repeat(4_096)).unwrap(); // ≈1024 tokens

        let ctx = Arc::new(Mutex::new(CompactWorkerContext::with_budget(100)));
        let tool: Arc<dyn Tool> = Arc::new(MarkReadRequiredTool {
            session: make_fs(tmp.path()),
            ctx: ctx.clone(),
        });
        let input = serde_json::json!({ "file_path": path.file_name().unwrap().to_str().unwrap() })
            .to_string();
        let res = tool.execute(&input, Default::default()).await;

        assert!(matches!(res, Err(ToolError::ExecutionFailed(_))));
        let guard = ctx.lock().unwrap();
        assert!(guard.read_required.is_empty());
        assert_eq!(guard.auto_read_consumed, 0);
    }

    #[tokio::test]
    async fn write_summary_overwrites_previous_call() {
        let ctx = Arc::new(Mutex::new(CompactWorkerContext::with_budget(0)));
        let tool: Arc<dyn Tool> = Arc::new(WriteSummaryTool { ctx: ctx.clone() });

        let first = serde_json::json!({ "text": "first" }).to_string();
        let out1 = tool.execute(&first, Default::default()).await.unwrap();
        assert!(out1.summary.contains("recorded"));

        let second = serde_json::json!({ "text": "second" }).to_string();
        let out2 = tool.execute(&second, Default::default()).await.unwrap();
        assert!(out2.summary.contains("replaced"));

        assert_eq!(ctx.lock().unwrap().summary.as_deref(), Some("second"));
    }

    #[tokio::test]
    async fn add_reference_deduplicates() {
        let ctx = Arc::new(Mutex::new(CompactWorkerContext::with_budget(0)));
        let tool: Arc<dyn Tool> = Arc::new(AddReferenceTool { ctx: ctx.clone() });

        let p = "/abs/path.rs";
        let input = serde_json::json!({ "file_path": p }).to_string();
        tool.execute(&input, Default::default()).await.unwrap();
        tool.execute(&input, Default::default()).await.unwrap();

        let guard = ctx.lock().unwrap();
        assert_eq!(guard.references.len(), 1);
        assert_eq!(guard.references[0], PathBuf::from(p));
    }

    #[tokio::test]
    async fn search_session_log_returns_bounded_hits_without_full_tool_content() {
        let items = Arc::new(vec![
            Item::user_message("investigate compact failure"),
            Item::tool_result_with_content(
                "call-1",
                "read trace with compact failure",
                "very large raw trace body with secret detail",
            ),
        ]);
        let view = SessionReferenceView::new("test", (*items).clone());
        let tool: Arc<dyn Tool> = Arc::new(SearchSessionLogTool {
            state: Arc::new(SessionLogToolState { items, view }),
        });
        let input = serde_json::json!({ "query": "compact", "limit": 10 }).to_string();
        let out = tool.execute(&input, Default::default()).await.unwrap();
        let content = out.content.unwrap();

        assert!(content.contains("investigate compact failure"));
        assert!(content.contains("read trace with compact failure"));
        assert!(!content.contains("secret detail"));
    }

    #[tokio::test]
    async fn read_session_items_full_mode_can_read_tool_result_content() {
        let items = Arc::new(vec![Item::tool_result_with_content(
            "call-1",
            "read trace",
            "raw trace detail",
        )]);
        let view = SessionReferenceView::new("test", (*items).clone());
        let tool: Arc<dyn Tool> = Arc::new(ReadSessionItemsTool {
            state: Arc::new(SessionLogToolState { items, view }),
        });
        let input = serde_json::json!({ "offset": 0, "limit": 1, "mode": "full" }).to_string();
        let out = tool.execute(&input, Default::default()).await.unwrap();
        let content = out.content.unwrap();

        assert!(content.contains("raw trace detail"));
    }

    #[test]
    fn slice_lines_handles_offset_and_limit() {
        let text = "a\nb\nc\nd";
        assert_eq!(slice_lines(text, 0, None), "a\nb\nc\nd");
        assert_eq!(slice_lines(text, 1, Some(2)), "b\nc");
        assert_eq!(slice_lines(text, 10, None), "");
    }
}
