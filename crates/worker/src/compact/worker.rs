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

use agen::Item;
use agen::interceptor::{Interceptor, PreRequestAction, PreToolAction, ToolCallInfo};
use agen::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput, ToolResult};
use async_trait::async_trait;
use serde::Deserialize;
#[cfg(test)]
use workdir::LocalWorkdirSession;
use workdir::{ReadRequest, WorkdirPath, WorkdirSessionHandle};

use crate::compact::usage_tracker::UsageTracker;
use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureModule, ToolContribution,
    ToolDeclaration,
};
use crate::fs_view::ReadRequirement;

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

const MARK_DESCRIPTION: &str = "Inject a file's contents into the compacted context so the \
next session starts with it already read. Use this for files the next task needs in full. \
Optionally specify `offset` (0-based line) and `limit` (line count) to inject only a slice. \
Counts against `auto_read_budget`; overflow returns an error and the mark is not recorded.";

const REFERENCE_DESCRIPTION: &str = "Record a Workdir-relative file path as a named reference in \
the compacted context without injecting its contents. Use for files that are contextually \
relevant but whose current content the next session can fetch on demand.";

const SUMMARY_DESCRIPTION: &str = "Provide the final structured summary text. Subsequent calls \
replace the previous content; only the last call is used. Must be called before the compact run \
ends or compaction fails.";

struct MarkReadRequiredTool {
    session: WorkdirSessionHandle,
    ctx: Arc<Mutex<CompactWorkerContext>>,
}

#[async_trait]
impl Tool for MarkReadRequiredTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
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

            attachments: Vec::new(),
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
        _ctx: agen::tool::ToolExecutionContext,
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

            attachments: Vec::new(),
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
        _ctx: agen::tool::ToolExecutionContext,
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

            attachments: Vec::new(),
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

#[derive(Clone)]
pub(crate) struct CompactionOutputFeature {
    session: Option<WorkdirSessionHandle>,
    tracker: tools::Tracker,
    context: Arc<Mutex<CompactWorkerContext>>,
}

impl CompactionOutputFeature {
    pub(crate) fn new(
        session: Option<WorkdirSessionHandle>,
        tracker: tools::Tracker,
        context: Arc<Mutex<CompactWorkerContext>>,
    ) -> Self {
        Self {
            session,
            tracker,
            context,
        }
    }
}

impl FeatureModule for CompactionOutputFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        let descriptor = FeatureDescriptor::builtin("compaction-output", "Compaction Output")
            .with_description("Read-only Workdir access and bounded compaction output decisions.")
            .with_tool(ToolDeclaration::new("add_reference", REFERENCE_DESCRIPTION))
            .with_tool(ToolDeclaration::new("write_summary", SUMMARY_DESCRIPTION));
        if self.session.is_some() {
            descriptor
                .with_tool(ToolDeclaration::new("Read", "Read a Workdir file."))
                .with_tool(ToolDeclaration::new("mark_read_required", MARK_DESCRIPTION))
        } else {
            descriptor
        }
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        if let Some(session) = &self.session {
            context.tools().register(ToolContribution::new(
                "Read",
                tools::read_tool(session.clone(), self.tracker.clone()),
            ))?;
            context.tools().register(ToolContribution::new(
                "mark_read_required",
                mark_read_required_tool(session.clone(), self.context.clone()),
            ))?;
        }
        context.tools().register(ToolContribution::new(
            "add_reference",
            add_reference_tool(self.context.clone()),
        ))?;
        context.tools().register(ToolContribution::new(
            "write_summary",
            write_summary_tool(self.context.clone()),
        ))?;
        Ok(())
    }
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
        let estimate = agen::token_counter::total_tokens(context, &records);
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

    fn make_usage(input: u64) -> agen::timeline::event::UsageEvent {
        agen::timeline::event::UsageEvent {
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
}
