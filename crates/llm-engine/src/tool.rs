//! Tool Definition
//!
//! Traits for defining tools callable by LLM.
//! Usually auto-implemented using the `#[tool]` macro.

use std::{collections::HashMap, fmt, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
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

/// Request-local binary payload emitted by a tool.
///
/// Attachments are deliberately excluded from serde. They may be projected into
/// the immediately following provider request, but never into persisted history,
/// protocol events, logs, or telemetry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attachment {
    Image(ImageAttachment),
}

/// Tool execution result.
///
/// Every output has a mandatory `summary` (1-2 lines) that persists in
/// conversation history even after pruning. The optional `content` carries
/// full text details. `attachments` are request-local and are never serialized.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    /// Short summary (1-2 lines). Always remains in history.
    pub summary: String,
    /// Detailed text output. Removed by Prune when old enough.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Structured binary payloads for the immediately following model request.
    #[serde(skip, default)]
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
/// use llm_engine::tool::{Tool, ToolError, ToolExecutionContext, ToolMeta, ToolDefinition, ToolOutput};
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
    /// Short summary (always kept in history)
    pub summary: String,
    /// Detailed output (prunable)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Whether this is an error
    #[serde(default)]
    pub is_error: bool,
    /// Request-local structured payloads. Never serialized or persisted.
    #[serde(skip, default)]
    pub attachments: Vec<Attachment>,
}

impl ToolResult {
    /// Create a success result from a [`ToolOutput`].
    pub fn from_output(tool_use_id: impl Into<String>, output: ToolOutput) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            summary: output.summary,
            content: output.content,
            is_error: false,
            attachments: output.attachments,
        }
    }

    /// Create an error result.
    pub fn error(tool_use_id: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            summary: message.into(),
            content: None,
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
