//! Hook-related type definitions
//!
//! Types used for turn control and intervention in the Worker layer

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

// =============================================================================
// Hook Event Kinds
// =============================================================================

pub trait HookEventKind: Send + Sync + 'static {
    type Input;
    type Output;
}

pub struct OnPromptSubmit;
pub struct PreLlmRequest;
pub struct PreToolCall;
pub struct PostToolCall;
pub struct OnTurnEnd;
pub struct OnAbort;
pub struct OnTextDelta;
pub struct OnToolCallDelta;
pub struct OnStreamChunk;
pub struct OnStreamComplete;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnPromptSubmitResult {
    Continue,
    Cancel(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreLlmRequestResult {
    Continue,
    Cancel(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreToolCallResult {
    Continue,
    Skip,
    Abort(String),
    Pause,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostToolCallResult {
    Continue,
    Abort(String),
}

#[derive(Debug, Clone)]
pub enum OnTurnEndResult {
    Finish,
    ContinueWithMessages(Vec<crate::Item>),
    Paused,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamHookResult {
    Continue,
    Abort(String),
    Pause,
}

use std::sync::Arc;

use crate::tool::{Tool, ToolMeta};

/// Input context for PreToolCall
pub struct ToolCallContext {
    /// Tool call information (modifiable)
    pub call: ToolCall,
    /// Tool meta information (immutable)
    pub meta: ToolMeta,
    /// Tool instance (for state access)
    pub tool: Arc<dyn Tool>,
}

/// Input context for PostToolCall
pub struct PostToolCallContext {
    /// Tool call information
    pub call: ToolCall,
    /// Tool execution result (modifiable)
    pub result: ToolResult,
    /// Tool meta information (immutable)
    pub meta: ToolMeta,
    /// Tool instance (for state access)
    pub tool: Arc<dyn Tool>,
}

/// Input context for OnTextDelta
#[derive(Debug, Clone)]
pub struct TextDeltaContext {
    /// Block index
    pub index: usize,
    /// Text delta content
    pub delta: String,
}

/// Input context for OnToolCallDelta
#[derive(Debug, Clone)]
pub struct ToolCallDeltaContext {
    /// Block index
    pub index: usize,
    /// Partial JSON fragment
    pub delta_json_fragment: String,
}

/// Input context for OnStreamChunk
#[derive(Debug, Clone)]
pub struct StreamChunkContext {
    /// Public worker-level event
    pub event: crate::event::Event,
}

/// Input context for OnStreamComplete
#[derive(Debug, Clone)]
pub struct StreamCompleteContext {
    /// Current turn number
    pub turn: usize,
    /// Number of streamed events in this request
    pub event_count: usize,
}

impl HookEventKind for OnPromptSubmit {
    type Input = crate::Item;
    type Output = OnPromptSubmitResult;
}

impl HookEventKind for PreLlmRequest {
    type Input = Vec<crate::Item>;
    type Output = PreLlmRequestResult;
}

impl HookEventKind for PreToolCall {
    type Input = ToolCallContext;
    type Output = PreToolCallResult;
}

impl HookEventKind for PostToolCall {
    type Input = PostToolCallContext;
    type Output = PostToolCallResult;
}

impl HookEventKind for OnTurnEnd {
    type Input = Vec<crate::Item>;
    type Output = OnTurnEndResult;
}

impl HookEventKind for OnAbort {
    type Input = String;
    type Output = ();
}

impl HookEventKind for OnTextDelta {
    type Input = TextDeltaContext;
    type Output = StreamHookResult;
}

impl HookEventKind for OnToolCallDelta {
    type Input = ToolCallDeltaContext;
    type Output = StreamHookResult;
}

impl HookEventKind for OnStreamChunk {
    type Input = StreamChunkContext;
    type Output = StreamHookResult;
}

impl HookEventKind for OnStreamComplete {
    type Input = StreamCompleteContext;
    type Output = StreamHookResult;
}

// =============================================================================
// Tool Call / Result Types
// =============================================================================

/// Tool call information
///
/// Represents a ToolUse block from LLM, modifiable in Hook processing
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
/// Represents the result after tool execution, modifiable in Hook processing
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

// =============================================================================
// Hook Error
// =============================================================================

/// Hook error
#[derive(Debug, Error)]
pub enum HookError {
    /// Processing was aborted
    #[error("Aborted: {0}")]
    Aborted(String),
    /// Internal error
    #[error("Hook error: {0}")]
    Internal(String),
}

// =============================================================================
// Hook Trait
// =============================================================================

/// Trait for handling Hook events
///
/// Each event type has a different return type, constrained via `HookEventKind`.
#[async_trait]
pub trait Hook<E: HookEventKind>: Send + Sync {
    async fn call(&self, input: &mut E::Input) -> Result<E::Output, HookError>;
}

// =============================================================================
// Hook Registry
// =============================================================================

/// Registry holding all Hooks
///
/// Used internally by Worker to manage all Hook types.
pub struct HookRegistry {
    /// on_prompt_submit Hook
    pub(crate) on_prompt_submit: Vec<Box<dyn Hook<OnPromptSubmit>>>,
    /// pre_llm_request Hook
    pub(crate) pre_llm_request: Vec<Box<dyn Hook<PreLlmRequest>>>,
    /// pre_tool_call Hook
    pub(crate) pre_tool_call: Vec<Box<dyn Hook<PreToolCall>>>,
    /// post_tool_call Hook
    pub(crate) post_tool_call: Vec<Box<dyn Hook<PostToolCall>>>,
    /// on_turn_end Hook
    pub(crate) on_turn_end: Vec<Box<dyn Hook<OnTurnEnd>>>,
    /// on_abort Hook
    pub(crate) on_abort: Vec<Box<dyn Hook<OnAbort>>>,
    /// on_text_delta Hook
    pub(crate) on_text_delta: Vec<Box<dyn Hook<OnTextDelta>>>,
    /// on_tool_call_delta Hook
    pub(crate) on_tool_call_delta: Vec<Box<dyn Hook<OnToolCallDelta>>>,
    /// on_stream_chunk Hook
    pub(crate) on_stream_chunk: Vec<Box<dyn Hook<OnStreamChunk>>>,
    /// on_stream_complete Hook
    pub(crate) on_stream_complete: Vec<Box<dyn Hook<OnStreamComplete>>>,
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl HookRegistry {
    /// Create an empty HookRegistry
    pub fn new() -> Self {
        Self {
            on_prompt_submit: Vec::new(),
            pre_llm_request: Vec::new(),
            pre_tool_call: Vec::new(),
            post_tool_call: Vec::new(),
            on_turn_end: Vec::new(),
            on_abort: Vec::new(),
            on_text_delta: Vec::new(),
            on_tool_call_delta: Vec::new(),
            on_stream_chunk: Vec::new(),
            on_stream_complete: Vec::new(),
        }
    }
}
