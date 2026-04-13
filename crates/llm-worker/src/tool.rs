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
// ToolOutput - Tool execution result with summary + content
// =============================================================================

/// Threshold below which tool output is treated as summary-only (no content).
/// Outputs this small don't benefit from pruning.
pub const SUMMARY_THRESHOLD: usize = 200;

/// Tool execution result.
///
/// Every output has a mandatory `summary` (1-2 lines) that persists in
/// conversation history even after pruning. The optional `content` carries
/// full details and is removed by the Prune mechanism when the context
/// grows too large.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    /// Short summary (1-2 lines). Always remains in history.
    pub summary: String,
    /// Detailed output. Removed by Prune when old enough.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

impl From<String> for ToolOutput {
    fn from(s: String) -> Self {
        if s.len() <= SUMMARY_THRESHOLD {
            ToolOutput {
                summary: s,
                content: None,
            }
        } else {
            let lines = s.lines().count();
            let first_line: String = s.lines().next().unwrap_or("").chars().take(80).collect();
            let summary = format!("{lines} lines | {first_line}…");
            ToolOutput {
                summary,
                content: Some(s),
            }
        }
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
    /// Execute the tool.
    ///
    /// # Arguments
    /// * `input_json` - JSON-formatted arguments generated by LLM
    ///
    /// # Returns
    /// A [`ToolOutput`] with summary and optional detailed content.
    /// For simple cases, use `From<String>`: `Ok("done".to_string().into())`
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError>;
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

impl ToolResult {
    /// Create a success result from a [`ToolOutput`].
    pub fn from_output(tool_use_id: impl Into<String>, output: ToolOutput) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            summary: output.summary,
            content: output.content,
            is_error: false,
        }
    }

    /// Create an error result.
    pub fn error(tool_use_id: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            summary: message.into(),
            content: None,
            is_error: true,
        }
    }
}
