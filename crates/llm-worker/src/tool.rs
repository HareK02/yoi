//! Tool Definition
//!
//! Traits for defining tools callable by LLM.
//! Usually auto-implemented using the `#[tool]` macro.

use std::sync::Arc;

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
// ToolOutput - Tool execution result with size-aware storage
// =============================================================================

/// Tool output size threshold in bytes.
/// Results larger than this are automatically promoted to `Stored`.
pub const INLINE_THRESHOLD: usize = 800;

/// Maximum size of auto-generated summaries in bytes.
pub const SUMMARY_MAX_BYTES: usize = 400;

/// Number of lines to include from the head of text content in summaries.
pub const SUMMARY_HEAD_LINES: usize = 5;

/// Number of lines to include from the tail of text content in summaries.
pub const SUMMARY_TAIL_LINES: usize = 3;

/// Tool execution result.
///
/// Small results are kept inline in conversation history.
/// Large results are stored externally via `BlobStore`, with only
/// a summary placed in the history. The LLM can retrieve details
/// using the built-in `inspect` tool.
#[derive(Debug, Clone)]
pub enum ToolOutput {
    /// Small result: placed directly into history as-is.
    Inline(String),
    /// Large result: summary goes into history, full content is stored externally.
    Stored {
        /// Concise summary shown to the LLM in conversation context.
        summary: String,
        /// Full content to be persisted in a BlobStore.
        content: Content,
    },
}

impl ToolOutput {
    /// Get the string that should be placed into conversation history.
    pub fn history_text(&self) -> &str {
        match self {
            ToolOutput::Inline(s) => s,
            ToolOutput::Stored { summary, .. } => summary,
        }
    }

    /// Whether this output requires external storage.
    pub fn is_stored(&self) -> bool {
        matches!(self, ToolOutput::Stored { .. })
    }
}

/// Content to be stored in a BlobStore.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum Content {
    /// Plain text (file contents, search results, logs, etc.)
    Text(String),
    /// Structured JSON data (API responses, query results, etc.)
    Structured(Value),
}

impl From<String> for ToolOutput {
    fn from(s: String) -> Self {
        if s.len() <= INLINE_THRESHOLD {
            ToolOutput::Inline(s)
        } else {
            let summary = auto_summarize_text(&s);
            ToolOutput::Stored {
                summary,
                content: Content::Text(s),
            }
        }
    }
}

/// Generate a summary for any [`Content`] variant.
///
/// The blob ID prefix (`[blob:<id>]`) is NOT included here — it is
/// prepended by the Worker after the content is stored and an ID is assigned.
pub fn auto_summarize(content: &Content) -> String {
    match content {
        Content::Text(text) => auto_summarize_text(text),
        Content::Structured(value) => auto_summarize_structured(value),
    }
}

/// Generate a summary for plain text content.
fn auto_summarize_text(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();

    let mut summary = format!("text | {total} lines\n");

    // Head
    summary.push_str("── head ──\n");
    for line in lines.iter().take(SUMMARY_HEAD_LINES) {
        summary.push_str(line);
        summary.push('\n');
    }

    // Tail (only if there's content beyond head)
    if total > SUMMARY_HEAD_LINES + SUMMARY_TAIL_LINES {
        summary.push_str("── tail ──\n");
        let tail_start = total.saturating_sub(SUMMARY_TAIL_LINES);
        for line in &lines[tail_start..] {
            summary.push_str(line);
            summary.push('\n');
        }
    }

    // Truncate if summary itself is too large
    if summary.len() > SUMMARY_MAX_BYTES {
        summary.truncate(SUMMARY_MAX_BYTES);
        summary.push_str("…\n");
    }

    summary
}

/// Generate a summary for structured JSON content.
fn auto_summarize_structured(value: &Value) -> String {
    let mut summary = match value {
        Value::Array(arr) => {
            let mut s = format!("json_array | {} entries\n", arr.len());
            // Show schema from first element
            if let Some(first) = arr.first() {
                s.push_str("── schema ──\n");
                s.push_str(&describe_value_shape(first));
                s.push('\n');
            }
            // Show first 2 entries
            s.push_str("── head ──\n");
            for item in arr.iter().take(2) {
                if let Ok(json) = serde_json::to_string(item) {
                    s.push_str(&json);
                    s.push('\n');
                }
            }
            s
        }
        Value::Object(map) => {
            let mut s = format!("json_object | {} keys\n", map.len());
            s.push_str("── keys ──\n");
            for (key, val) in map.iter() {
                s.push_str(&format!("{key}: {}\n", value_type_label(val)));
            }
            s
        }
        _ => {
            // Scalar or other — just show the JSON
            format!(
                "json | {}\n",
                serde_json::to_string(value).unwrap_or_default()
            )
        }
    };

    if summary.len() > SUMMARY_MAX_BYTES {
        summary.truncate(SUMMARY_MAX_BYTES);
        summary.push_str("…\n");
    }

    summary
}

/// Describe the shape of a JSON value (for schema preview).
fn describe_value_shape(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let fields: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("{k}: {}", value_type_label(v)))
                .collect();
            format!("{{ {} }}", fields.join(", "))
        }
        _ => value_type_label(value),
    }
}

/// Human-readable type label for a JSON value.
fn value_type_label(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(_) => "bool".to_string(),
        Value::Number(_) => "number".to_string(),
        Value::String(s) => {
            if s.len() > 50 {
                format!("string({})", s.len())
            } else {
                "string".to_string()
            }
        }
        Value::Array(arr) => format!("array({})", arr.len()),
        Value::Object(map) => format!("object({})", map.len()),
    }
}

// =============================================================================
// ToolMeta - Immutable Meta Information
// =============================================================================

/// Tool meta information (fixed at registration, immutable)
///
/// Generated from `ToolDefinition` factory and does not change after registration with Worker.
/// Used for sending tool definitions to LLM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolMeta {
    /// Tool name (used by LLM for identification)
    pub name: String,
    /// Tool description (included in prompt to LLM)
    pub description: String,
    /// JSON Schema for arguments
    pub input_schema: Value,
}

impl ToolMeta {
    /// Create a new ToolMeta
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            input_schema: Value::Object(Default::default()),
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
}

// =============================================================================
// ToolDefinition - Factory Type
// =============================================================================

/// Tool definition factory
///
/// When called, returns `(ToolMeta, Arc<dyn Tool>)`.
/// Called once during Worker registration, and the meta information and instance
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
/// worker.register_tool(def)?;
/// ```
pub type ToolDefinition = Arc<dyn Fn() -> (ToolMeta, Arc<dyn Tool>) + Send + Sync>;

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
/// worker.register_tool(app.search_definition())?;
/// ```
///
/// # Manual Implementation
///
/// ```ignore
/// use llm_worker::tool::{Tool, ToolError, ToolMeta, ToolDefinition};
/// use std::sync::Arc;
///
/// struct MyTool { counter: std::sync::atomic::AtomicUsize }
///
/// #[async_trait::async_trait]
/// impl Tool for MyTool {
///     async fn execute(&self, input: &str) -> Result<String, ToolError> {
///         self.counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
///         Ok("result".to_string())
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
    /// Execute the tool
    ///
    /// # Arguments
    /// * `input_json` - JSON-formatted arguments generated by LLM
    ///
    /// # Returns
    /// Result string from execution. This content is returned to LLM.
    async fn execute(&self, input_json: &str) -> Result<String, ToolError>;
}

// =============================================================================
// ToolOutputProcessor - Output storage abstraction
// =============================================================================

/// Processes tool output before it enters conversation history.
///
/// When a tool produces a large result, the processor can store the
/// full content externally and return a summary string for the history.
///
/// If no processor is set on Worker, all tool outputs are used as-is (inline).
#[async_trait]
pub trait ToolOutputProcessor: Send + Sync {
    /// Process a tool's raw output string.
    ///
    /// Returns the string that should be placed into conversation history.
    /// For small outputs, this may be the original string unchanged.
    /// For large outputs, this should be a summary with a blob reference.
    async fn process(&self, output: String) -> Result<String, ToolError>;
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
/// Represents the result after tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// Corresponding tool call ID
    pub tool_use_id: String,
    /// Result content
    pub content: String,
    /// Whether this is an error
    #[serde(default)]
    pub is_error: bool,
}

impl ToolResult {
    /// Create a success result
    pub fn success(tool_use_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            content: content.into(),
            is_error: false,
        }
    }

    /// Create an error result
    pub fn error(tool_use_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            content: content.into(),
            is_error: true,
        }
    }
}
