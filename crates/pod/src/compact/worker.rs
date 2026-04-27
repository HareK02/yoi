//! Compact worker state and the four tools that drive it.
//!
//! The compact worker is a disposable `Worker` instance spun up by
//! [`Pod::compact`]. It receives the history to summarise plus a list of
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
//! which `Pod::compact` drains after the loop and turns into the
//! compacted session's opening system messages.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use llm_worker::Item;
use llm_worker::interceptor::{Interceptor, PreRequestAction};
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use serde::Deserialize;
use tools::ScopedFs;

/// A file the compact worker has marked for auto-read in the new session.
#[derive(Debug, Clone)]
pub(crate) struct ReadRequirement {
    pub path: PathBuf,
    /// 0-based line offset. `None` means from the start of the file.
    pub offset: Option<usize>,
    /// Maximum number of lines. `None` means to the end of the file.
    pub limit: Option<usize>,
}

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
Counts against `auto_read_budget`; overflow returns an error and the mark is not recorded. \
Paths must be absolute.";

const REFERENCE_DESCRIPTION: &str = "Record a file path as a named reference in the compacted \
context without injecting its contents. Use for files that are contextually relevant but \
whose current content the next session can fetch on demand.";

const SUMMARY_DESCRIPTION: &str = "Provide the final structured summary text. Subsequent calls \
replace the previous content; only the last call is used. Must be called before the compact run \
ends or compaction fails.";

struct MarkReadRequiredTool {
    fs: ScopedFs,
    ctx: Arc<Mutex<CompactWorkerContext>>,
}

#[async_trait]
impl Tool for MarkReadRequiredTool {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let params: MarkParams = serde_json::from_str(input_json).map_err(|e| {
            ToolError::InvalidArgument(format!("invalid mark_read_required input: {e}"))
        })?;

        // Read the file through the shared ScopedFs so scope and I/O
        // errors surface the same way the regular `read_file` tool does.
        let bytes = self
            .fs
            .read_bytes(&params.file_path)
            .map_err(|e| ToolError::ExecutionFailed(format!("read failed: {e}")))?;
        let text = String::from_utf8_lossy(&bytes);
        let slice = slice_lines(&text, params.offset.unwrap_or(0), params.limit);
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
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
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
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
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
    fs: ScopedFs,
    ctx: Arc<Mutex<CompactWorkerContext>>,
) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(MarkParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("mark_read_required")
            .description(MARK_DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(MarkReadRequiredTool {
            fs: fs.clone(),
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

/// Interceptor that aborts the compact worker as soon as its cumulative
/// input-token count crosses `max_input_tokens`. Pairs with the
/// `on_usage` callback registered by `Pod::compact`, which is what
/// actually accumulates `input_so_far`.
pub(crate) struct CompactWorkerInterceptor {
    pub input_so_far: Arc<AtomicU64>,
    pub max_input_tokens: u64,
}

#[async_trait]
impl Interceptor for CompactWorkerInterceptor {
    async fn pre_llm_request(&self, _context: &mut Vec<Item>) -> PreRequestAction {
        if self.input_so_far.load(Ordering::Relaxed) > self.max_input_tokens {
            return PreRequestAction::Cancel(format!(
                "compact worker input exceeded {} tokens",
                self.max_input_tokens
            ));
        }
        PreRequestAction::Continue
    }
}

/// Crude bytes→tokens estimate; good enough for budget accounting.
fn estimate_tokens(bytes: usize) -> u64 {
    (bytes as u64).div_ceil(4)
}

/// Return the slice of `text` covered by `offset` (line index) and
/// optional `limit` (line count), preserving the original newline
/// separation. Returns the whole file when both defaults apply.
pub(crate) fn slice_lines(text: &str, offset: usize, limit: Option<usize>) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = offset.min(lines.len());
    let end = limit
        .map(|n| start.saturating_add(n).min(lines.len()))
        .unwrap_or(lines.len());
    lines[start..end].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use manifest::Scope;

    fn make_fs(tmp: &std::path::Path) -> ScopedFs {
        let scope = Scope::writable(tmp.to_path_buf()).unwrap();
        ScopedFs::new(scope, tmp.to_path_buf())
    }

    #[tokio::test]
    async fn mark_read_required_records_and_deducts_budget() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("hello.txt");
        std::fs::write(&path, "hello world\n").unwrap();

        let ctx = Arc::new(Mutex::new(CompactWorkerContext::with_budget(1_000)));
        let tool: Arc<dyn Tool> = Arc::new(MarkReadRequiredTool {
            fs: make_fs(tmp.path()),
            ctx: ctx.clone(),
        });
        let input = serde_json::json!({ "file_path": path.to_str().unwrap() }).to_string();
        let out = tool.execute(&input).await.unwrap();

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
            fs: make_fs(tmp.path()),
            ctx: ctx.clone(),
        });
        let input = serde_json::json!({ "file_path": path.to_str().unwrap() }).to_string();
        let res = tool.execute(&input).await;

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
        let out1 = tool.execute(&first).await.unwrap();
        assert!(out1.summary.contains("recorded"));

        let second = serde_json::json!({ "text": "second" }).to_string();
        let out2 = tool.execute(&second).await.unwrap();
        assert!(out2.summary.contains("replaced"));

        assert_eq!(ctx.lock().unwrap().summary.as_deref(), Some("second"));
    }

    #[tokio::test]
    async fn add_reference_deduplicates() {
        let ctx = Arc::new(Mutex::new(CompactWorkerContext::with_budget(0)));
        let tool: Arc<dyn Tool> = Arc::new(AddReferenceTool { ctx: ctx.clone() });

        let p = "/abs/path.rs";
        let input = serde_json::json!({ "file_path": p }).to_string();
        tool.execute(&input).await.unwrap();
        tool.execute(&input).await.unwrap();

        let guard = ctx.lock().unwrap();
        assert_eq!(guard.references.len(), 1);
        assert_eq!(guard.references[0], PathBuf::from(p));
    }

    #[test]
    fn slice_lines_handles_offset_and_limit() {
        let text = "a\nb\nc\nd";
        assert_eq!(slice_lines(text, 0, None), "a\nb\nc\nd");
        assert_eq!(slice_lines(text, 1, Some(2)), "b\nc");
        assert_eq!(slice_lines(text, 10, None), "");
    }
}
