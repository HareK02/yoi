//! Tool Definition
//!
//! Traits for defining tools callable by LLM.
//! Usually auto-implemented using the `#[tool]` macro.

use std::{
    collections::HashMap,
    fmt,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use serde_json::Value;
use thiserror::Error;

/// Error during tool execution
#[derive(Debug, Error)]
pub enum ToolError {
    /// Invalid argument
    #[error("Invalid argument: {0}")]
    InvalidArgument(String),
    /// Execution failed
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),
    /// Internal error
    #[error("Internal error: {0}")]
    Internal(String),
    /// Cooperative cancellation completed with bounded terminal output.
    #[error("Tool execution cancelled")]
    Cancelled(ToolOutput),
    /// Execution was interrupted with a confirmed bounded terminal output.
    #[error("Tool execution interrupted")]
    Interrupted(ToolOutput),
}

// =============================================================================
// ToolOutput - Tool execution result with summary + content
// =============================================================================

/// Threshold below which tool output is treated as summary-only (no content).
/// Outputs this small don't benefit from pruning.
pub const SUMMARY_THRESHOLD: usize = 200;

/// Byte-size caps applied to tool execution `content` at the Engine's
/// tool-execution boundary, before results enter conversation history.
///
/// Exists so a single oversized tool result (e.g. a wide `Glob` scan)
/// cannot blow past the provider's per-minute input-token rate limit.
/// Individual tools are not trusted to self-limit — this is the single
/// chokepoint.
///
/// The unit is bytes rather than tokens because accurate pre-send token
/// estimation is not available. The limits can be migrated to token
/// units later without changing callers.
#[derive(Debug, Clone)]
pub struct ToolOutputLimits {
    /// Cap applied to any tool not listed in `per_tool`.
    pub default_max_bytes: usize,
    /// Per-tool overrides, keyed by tool registration name.
    pub per_tool: HashMap<String, usize>,
}

impl ToolOutputLimits {
    /// Resolve the cap for a given tool name.
    pub fn limit_for(&self, tool_name: &str) -> usize {
        self.per_tool
            .get(tool_name)
            .copied()
            .unwrap_or(self.default_max_bytes)
    }
}

/// Truncate `content` in-place if it exceeds `limit` bytes, replacing
/// the dropped tail with a short human- and LLM-readable marker so the
/// model can self-correct by narrowing its query.
///
/// The cut point is walked back to the nearest UTF-8 char boundary so
/// multibyte characters are never split.
pub(crate) fn truncate_content(content: &mut String, limit: usize) {
    let original_len = content.len();
    if original_len <= limit {
        return;
    }

    let suffix_template = "\n\n[truncated: %BYTES% bytes dropped, refine your query]";
    // Reserve enough headroom for the suffix (upper bound on the byte length
    // of the number substitution). usize::MAX fits in 20 digits.
    let reserved = suffix_template.len() + 20 - "%BYTES%".len();
    let body_budget = limit.saturating_sub(reserved);

    let mut cut = body_budget.min(original_len);
    while cut > 0 && !content.is_char_boundary(cut) {
        cut -= 1;
    }
    content.truncate(cut);
    let dropped = original_len - cut;
    content.push_str(&suffix_template.replace("%BYTES%", &dropped.to_string()));
}

#[derive(Clone, PartialEq, Eq)]
pub struct ImageAttachment {
    mime_type: String,
    data: Arc<[u8]>,
}

impl ImageAttachment {
    pub fn new(mime_type: impl Into<String>, data: impl Into<Arc<[u8]>>) -> Self {
        Self {
            mime_type: mime_type.into(),
            data: data.into(),
        }
    }

    pub fn mime_type(&self) -> &str {
        &self.mime_type
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

impl fmt::Debug for ImageAttachment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImageAttachment")
            .field("mime_type", &self.mime_type)
            .field("bytes", &self.data.len())
            .finish()
    }
}

#[derive(Serialize, Deserialize)]
struct ImageAttachmentWire {
    mime_type: String,
    data: String,
}

impl Serialize for ImageAttachment {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        ImageAttachmentWire {
            mime_type: self.mime_type.clone(),
            data: STANDARD.encode(self.data.as_ref()),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ImageAttachment {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ImageAttachmentWire::deserialize(deserializer)?;
        let data = STANDARD.decode(wire.data).map_err(D::Error::custom)?;
        Ok(Self::new(wire.mime_type, data))
    }
}

/// Durable binary detail emitted by a tool and handled by normal ToolResult pruning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum Attachment {
    Image(ImageAttachment),
}

/// Terminal disposition of one started tool call.
///
/// `Cancelled` means the tool confirmed cancellation. `OutcomeUnknown` means
/// execution stopped without confirmation, so neither completion nor side
/// effects may be inferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolResultDisposition {
    #[default]
    Success,
    Error,
    Interrupted,
    Cancelled,
    OutcomeUnknown,
}

impl ToolResultDisposition {
    pub const fn is_success(&self) -> bool {
        matches!(self, Self::Success)
    }
}

/// Tool execution result.
///
/// Every output has a mandatory `summary` (1-2 lines) that persists in
/// conversation history even after pruning. Optional text and binary details are
/// committed to history and may later be omitted only by normal ToolResult pruning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    /// Short summary (1-2 lines). Always remains in history.
    pub summary: String,
    /// Detailed text output. Removed by Prune when old enough.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Durable binary details handled by the same pruning lifecycle as `content`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
}

impl From<String> for ToolOutput {
    fn from(s: String) -> Self {
        if s.len() <= SUMMARY_THRESHOLD {
            ToolOutput {
                summary: s,
                content: None,
                attachments: Vec::new(),
            }
        } else {
            let lines = s.lines().count();
            let first_line: String = s.lines().next().unwrap_or("").chars().take(80).collect();
            let summary = format!("{lines} lines | {first_line}…");
            ToolOutput {
                summary,
                content: Some(s),
                attachments: Vec::new(),
            }
        }
    }
}

// =============================================================================
// ToolMeta - Immutable Meta Information
// =============================================================================

/// Origin metadata for a registered tool.
///
/// This metadata is intentionally not part of the provider-facing tool schema.
/// It lets host layers audit where a model-visible tool definition came from
/// while keeping execution and permission semantics in the normal Engine path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolOrigin {
    /// Origin kind, for example `plugin` or `builtin`.
    pub kind: String,
    /// Package-local plugin id.
    pub plugin_id: String,
    /// Source-qualified plugin/package reference when `kind == "plugin"`.
    pub plugin_ref: String,
    /// Plugin source such as `user`, `project`, or `builtin`.
    pub source: String,
    /// Resolved package digest.
    pub digest: String,
    /// Resolved package version.
    pub package_version: String,
    /// Plugin API/schema version declared by the package.
    pub package_api_version: u32,
    /// Surface that contributed this tool. Plugin tools use `tool`.
    pub surface: String,
}

/// Tool meta information (fixed at registration, immutable)
///
/// Generated from `ToolDefinition` factory and does not change after registration with Engine.
/// Used for sending tool definitions to LLM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolMeta {
    /// Tool name (used by LLM for identification)
    pub name: String,
    /// Tool description (included in prompt to LLM)
    pub description: String,
    /// JSON Schema for arguments
    pub input_schema: Value,
    /// Optional host-side origin metadata. This is not exposed to the LLM.
    pub origin: Option<ToolOrigin>,
}

impl ToolMeta {
    /// Create a new ToolMeta
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            input_schema: Value::Object(Default::default()),
            origin: None,
        }
    }

    /// Set the description
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = desc.into();
        self
    }

    /// Set the argument schema
    pub fn input_schema(mut self, schema: Value) -> Self {
        self.input_schema = schema;
        self
    }

    /// Set host-side origin metadata.
    pub fn origin(mut self, origin: ToolOrigin) -> Self {
        self.origin = Some(origin);
        self
    }
}

// =============================================================================
// ToolDefinition - Factory Type
// =============================================================================

/// Tool definition factory
///
/// When called, returns `(ToolMeta, Arc<dyn Tool>)`.
/// Called once during Engine registration, and the meta information and instance
/// are cached at session scope.
///
/// # Examples
///
/// ```ignore
/// let def: ToolDefinition = Arc::new(|| {
///     (
///         ToolMeta::new("my_tool")
///             .description("My tool description")
///             .input_schema(json!({"type": "object"})),
///         Arc::new(MyToolImpl { state: 0 }) as Arc<dyn Tool>,
///     )
/// });
/// engine.register_tool(def)?;
/// ```
pub type ToolDefinition = Arc<dyn Fn() -> (ToolMeta, Arc<dyn Tool>) + Send + Sync>;

/// Per-call context supplied by the engine when executing a tool call.
///
/// The context identifies a tool call within one assistant response's tool-call
/// batch without imposing any scheduling policy on the engine. Tool
/// implementations may use it for response-local ordering, diagnostics, or
/// correlation, but it is intentionally not a handle to engine state, history,
/// or session mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExecutionContext {
    /// Provider/tool-call id for the call being executed.
    pub call_id: String,
    /// Engine-local identity shared by all tool calls from one execution batch.
    pub batch_id: String,
    /// Zero-based order of this call in the model-returned tool-call list.
    pub call_index: usize,
}

impl ToolExecutionContext {
    pub fn new(call_id: impl Into<String>, batch_id: impl Into<String>, call_index: usize) -> Self {
        Self {
            call_id: call_id.into(),
            batch_id: batch_id.into(),
            call_index,
        }
    }

    /// Identifies one live execution attempt without making the batch id a durable
    /// replay or idempotency authority.
    pub fn execution_id(&self) -> String {
        format!("{}:{}", self.batch_id, self.call_id)
    }

    /// Context for direct, non-engine calls in unit tests and low-level callers.
    pub fn direct() -> Self {
        Self::new("direct", "direct", 0)
    }
}

impl Default for ToolExecutionContext {
    fn default() -> Self {
        Self::direct()
    }
}

/// The provider-confirmed terminal result of one started tool execution.
///
/// `OutcomeUnknown` is reserved for an execution task that had to be force-closed
/// or failed before the provider could confirm its terminal result.
#[derive(Debug)]
pub enum ToolExecutionTerminal {
    Confirmed(Result<ToolOutput, ToolError>),
    OutcomeUnknown,
}

/// The completion future paired with a [`ToolExecutionHandle`]. Dropping this
/// future does not drop the provider execution: the spawned execution remains
/// owned by its handle until it completes or is explicitly force-closed.
pub struct ToolExecutionTerminalFuture {
    task: tokio::task::JoinHandle<Result<ToolOutput, ToolError>>,
}

impl Future for ToolExecutionTerminalFuture {
    type Output = ToolExecutionTerminal;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.task).poll(cx) {
            Poll::Ready(Ok(result)) => Poll::Ready(ToolExecutionTerminal::Confirmed(result)),
            Poll::Ready(Err(_)) => Poll::Ready(ToolExecutionTerminal::OutcomeUnknown),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Live ownership and control for one started tool execution.
///
/// Execution, cancellation, and terminal confirmation remain provider-owned:
/// this handle starts `Tool::execute`, delegates cooperative cancellation to
/// `Tool::cancel_execution`, and treats execution-future completion as the
/// provider's terminal confirmation. Agen may force-close only after its caller's
/// deadline expires, at which point the outcome is necessarily unknown.
#[derive(Clone)]
pub struct ToolExecutionHandle {
    inner: Arc<ToolExecutionHandleInner>,
}

struct ToolExecutionHandleInner {
    tool: Arc<dyn Tool>,
    context: ToolExecutionContext,
    abort: tokio::task::AbortHandle,
}

impl Drop for ToolExecutionHandleInner {
    fn drop(&mut self) {
        // Losing the final live owner is an explicit forced close, never a
        // best-effort detached provider future.
        self.abort.abort();
    }
}

impl fmt::Debug for ToolExecutionHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolExecutionHandle")
            .field("call_id", &self.inner.context.call_id)
            .field("batch_id", &self.inner.context.batch_id)
            .finish_non_exhaustive()
    }
}

impl ToolExecutionHandle {
    pub fn start(
        tool: Arc<dyn Tool>,
        input_json: String,
        context: ToolExecutionContext,
    ) -> (Self, ToolExecutionTerminalFuture) {
        let execution_tool = Arc::clone(&tool);
        let execution_context = context.clone();
        let task =
            tokio::spawn(
                async move { execution_tool.execute(&input_json, execution_context).await },
            );
        let abort = task.abort_handle();
        (
            Self {
                inner: Arc::new(ToolExecutionHandleInner {
                    tool,
                    context,
                    abort,
                }),
            },
            ToolExecutionTerminalFuture { task },
        )
    }

    pub fn context(&self) -> &ToolExecutionContext {
        &self.inner.context
    }

    pub async fn cancel_before(&self, deadline: tokio::time::Instant) -> Result<(), ToolError> {
        match tokio::time::timeout_at(
            deadline,
            self.inner.tool.cancel_execution(&self.inner.context),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(ToolError::Internal(format!(
                "tool cancellation request exceeded its deadline for call {}",
                self.inner.context.call_id
            ))),
        }
    }

    pub fn force_close(&self) {
        self.inner.abort.abort();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolExecutionPolicy {
    /// Maximum time allowed for a provider to accept one cooperative
    /// cancellation request.
    pub cancellation_request_timeout: std::time::Duration,
    /// Maximum time allowed for all providers to confirm terminal results after
    /// cancellation has been requested.
    pub terminal_confirmation_timeout: std::time::Duration,
}

impl Default for ToolExecutionPolicy {
    fn default() -> Self {
        Self {
            cancellation_request_timeout: std::time::Duration::from_millis(100),
            terminal_confirmation_timeout: std::time::Duration::from_millis(500),
        }
    }
}

// =============================================================================
// Tool trait
// =============================================================================

/// Trait for defining tools callable by LLM
///
/// Tools are used by LLM to access external resources
/// or execute computations.
/// Can maintain state during the session.
///
/// # How to Implement
///
/// Usually auto-implemented using the `#[tool_registry]` macro:
///
/// ```ignore
/// #[tool_registry]
/// impl MyApp {
///     #[tool]
///     async fn search(&self, query: String) -> String {
///         format!("Results for: {}", query)
///     }
/// }
///
/// // Register
/// engine.register_tool(app.search_definition())?;
/// ```
///
/// # Manual Implementation
///
/// ```ignore
/// use agen::tool::{Tool, ToolError, ToolExecutionContext, ToolMeta, ToolDefinition, ToolOutput};
/// use std::sync::Arc;
///
/// struct MyTool { counter: std::sync::atomic::AtomicUsize }
///
/// #[async_trait::async_trait]
/// impl Tool for MyTool {
///     async fn execute(&self, input: &str, ctx: ToolExecutionContext) -> Result<ToolOutput, ToolError> {
///         self.counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
///         Ok(format!("call {}: {}", ctx.call_index, input).into())
///     }
/// }
///
/// let def: ToolDefinition = Arc::new(|| {
///     (
///         ToolMeta::new("my_tool")
///             .description("My custom tool")
///             .input_schema(serde_json::json!({"type": "object"})),
///         Arc::new(MyTool { counter: Default::default() }) as Arc<dyn Tool>,
///     )
/// });
/// ```
#[async_trait]
pub trait Tool: Send + Sync {
    /// Execute the tool.
    ///
    /// # Arguments
    /// * `input_json` - JSON-formatted arguments generated by LLM
    /// * `ctx` - response-local call identity and ordering context
    ///
    /// # Returns
    /// A [`ToolOutput`] with summary and optional detailed content.
    /// For simple cases, use `From<String>`: `Ok("done".to_string().into())`
    async fn execute(
        &self,
        input_json: &str,
        ctx: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError>;

    /// Request cooperative cancellation for one started call.
    ///
    /// Implementations that own cancellable provider operations should signal
    /// every live execution identified by `call_id`, then let `execute` return
    /// the confirmed bounded terminal output. Direct callers may use this
    /// compatibility surface; Agen uses [`Tool::cancel_execution`] so providers
    /// can bind cancellation to one exact live attempt.
    async fn cancel(&self, _call_id: &str) -> Result<(), ToolError> {
        Ok(())
    }

    /// Request cooperative cancellation for one exact started execution.
    ///
    /// The default preserves existing tools by delegating to `cancel(call_id)`.
    /// Providers with their own execution registry should override this method
    /// and key cancellation by [`ToolExecutionContext::execution_id`].
    async fn cancel_execution(&self, ctx: &ToolExecutionContext) -> Result<(), ToolError> {
        self.cancel(&ctx.call_id).await
    }
}

// =============================================================================
// Tool Call / Result Types
// =============================================================================

/// Tool call information
///
/// Represents a ToolUse block from LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Tool call ID (used for linking with response)
    pub id: String,
    /// Tool name
    pub name: String,
    /// Input arguments (JSON)
    pub input: Value,
}

/// Tool execution result
///
/// Intermediate representation between tool execution and history.
/// Carries `summary` + optional `content` from [`ToolOutput`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolResult {
    /// Corresponding tool call ID
    pub tool_use_id: String,
    /// Typed terminal state.
    #[serde(default, skip_serializing_if = "ToolResultDisposition::is_success")]
    pub disposition: ToolResultDisposition,
    /// Short summary (always kept in history)
    pub summary: String,
    /// Detailed output (prunable)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Whether this is an error
    #[serde(default)]
    pub is_error: bool,
    /// Durable binary details (prunable with `content`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
}

impl ToolResult {
    /// Create a success result from a [`ToolOutput`].
    pub fn from_output(tool_use_id: impl Into<String>, output: ToolOutput) -> Self {
        Self::from_output_with_disposition(tool_use_id, output, ToolResultDisposition::Success)
    }

    pub fn from_output_with_disposition(
        tool_use_id: impl Into<String>,
        output: ToolOutput,
        disposition: ToolResultDisposition,
    ) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            disposition,
            summary: output.summary,
            content: output.content,
            is_error: !disposition.is_success(),
            attachments: output.attachments,
        }
    }

    /// Create an error result.
    pub fn error(tool_use_id: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            disposition: ToolResultDisposition::Error,
            summary: message.into(),
            content: None,
            is_error: true,
            attachments: Vec::new(),
        }
    }

    /// Close an execution whose completion and side effects cannot be confirmed.
    pub fn outcome_unknown(tool_use_id: impl Into<String>) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            disposition: ToolResultDisposition::OutcomeUnknown,
            summary: "Tool execution outcome unknown".to_string(),
            content: Some(
                "Execution was interrupted before completion could be confirmed. Completion and side effects are unknown."
                    .to_string(),
            ),
            is_error: true,
            attachments: Vec::new(),
        }
    }
}

#[cfg(test)]
mod truncate_tests {
    use super::*;

    #[test]
    fn noop_when_within_limit() {
        let mut s = "hello world".to_string();
        truncate_content(&mut s, 1024);
        assert_eq!(s, "hello world");
    }

    #[test]
    fn noop_at_exact_limit() {
        let mut s = "a".repeat(100);
        truncate_content(&mut s, 100);
        assert_eq!(s.len(), 100);
    }

    #[test]
    fn truncates_oversized_ascii_with_marker() {
        let mut s = "a".repeat(1000);
        truncate_content(&mut s, 200);
        assert!(s.contains("[truncated:"));
        assert!(s.contains("refine your query"));
        assert!(s.len() <= 200, "result was {} bytes", s.len());
        let dropped: usize = s
            .split("[truncated: ")
            .nth(1)
            .unwrap()
            .split(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let body_len = s.find("\n\n[truncated:").unwrap();
        assert_eq!(body_len + dropped, 1000);
    }

    #[test]
    fn respects_utf8_char_boundaries() {
        // 100 copies of "あ" (3 bytes each) = 300 bytes.
        let mut s = "あ".repeat(100);
        truncate_content(&mut s, 120);
        // Truncation must not split a multibyte character.
        assert!(s.is_char_boundary(s.find("\n\n[truncated:").unwrap_or(s.len())));
        // And the result must still be valid UTF-8 (implicitly true for String).
        assert!(s.contains("[truncated:"));
    }

    #[test]
    fn limits_per_tool_override() {
        let mut limits = ToolOutputLimits {
            default_max_bytes: 1024,
            per_tool: HashMap::new(),
        };
        limits.per_tool.insert("Read".to_string(), 4096);
        assert_eq!(limits.limit_for("Read"), 4096);
        assert_eq!(limits.limit_for("Grep"), 1024);
    }
}
