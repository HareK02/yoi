//! Pod-layer hook infrastructure
//!
//! Hooks are the **public** orchestration extension point. They receive
//! read-only summary information about each event in the Worker
//! execution loop and return a control-flow action
//! (continue / skip / abort / pause).
//!
//! Hooks intentionally cannot mutate the Worker's context, history, tool
//! call, or tool result. Internal mechanisms that need such access (e.g.
//! compaction, notification injection, output truncation) implement
//! `llm_worker::Interceptor` directly inside Pod, never via this trait.
//!
//! This separation lets Hooks be exposed safely to user-facing
//! extension surfaces (scripting, plugins) in the future without
//! exposing the underlying mutable state.

use async_trait::async_trait;
use llm_worker::tool::ToolOutput;
use llm_worker::interceptor::{
    PostToolAction, PreRequestAction, PreToolAction, PromptAction, TurnEndAction,
};
use serde_json::Value;

// =============================================================================
// Hook input summary types (read-only)
// =============================================================================

/// Information passed to `OnPromptSubmit` hooks.
pub struct PromptSubmitInfo {
    /// Concatenated text content of the user's input message.
    pub input_text: String,
    /// 0-based turn index this prompt opens.
    pub turn_index: usize,
}

/// Information passed to `PreLlmRequest` hooks.
pub struct PreRequestInfo {
    /// Number of items currently in the Worker context.
    pub item_count: usize,
    /// Most recently observed `input_tokens` from the LLM provider.
    /// `None` when the Pod has no compaction state attached, or when
    /// no LLM call has completed yet.
    pub estimated_tokens: Option<u64>,
    /// Current turn index (0-based).
    pub turn_index: usize,
    /// Tool calls already executed in this turn.
    pub tool_calls_this_turn: usize,
}

/// Information passed to `PreToolCall` hooks.
pub struct ToolCallSummary {
    /// Provider-assigned tool call id.
    pub call_id: String,
    /// Registered tool name.
    pub tool_name: String,
    /// Tool arguments as a JSON value (cloned).
    ///
    /// LLM-generated arguments are bounded by max_tokens, so cloning
    /// is cheap relative to tool execution. Structural access is
    /// required for permission decisions (e.g. inspecting a `path`
    /// field), which a stringified preview would not support.
    pub arguments: Value,
}

/// Information passed to `PostToolCall` hooks.
pub struct ToolResultSummary {
    /// Provider-assigned tool call id this result corresponds to.
    pub call_id: String,
    /// Registered tool name.
    pub tool_name: String,
    /// Whether the tool reported an error.
    pub is_error: bool,
    /// Tool output (`summary` always present, `content` may be `None`).
    pub output: ToolOutput,
}

/// Information passed to `OnTurnEnd` hooks.
pub struct TurnEndInfo {
    /// Turn that just ended (0-based).
    pub turn_index: usize,
    /// Tool calls executed in this turn.
    pub tool_calls_count: usize,
    /// Preview of the assistant's final text in this turn.
    /// Truncated at a UTF-8 boundary; empty when no assistant text exists.
    pub final_text_preview: String,
}

/// Information passed to `OnAbort` hooks.
pub struct AbortInfo {
    /// Reason supplied by the aborter.
    pub reason: String,
}

// =============================================================================
// Hook Event Kinds
// =============================================================================

/// Marker trait for hook event kinds.
///
/// Each event kind specifies its read-only input and the control-flow
/// action returned by hooks.
pub trait HookEventKind: Send + Sync + 'static {
    /// Read-only input passed to the hook.
    type Input: Send + Sync;
    /// Control-flow action returned by the hook.
    type Output;
}

/// After receiving user input, before adding to history.
pub struct OnPromptSubmit;
/// Before each LLM request.
pub struct PreLlmRequest;
/// Before each tool is executed.
pub struct PreToolCall;
/// After each tool completes.
pub struct PostToolCall;
/// When a turn ends with no tool calls.
pub struct OnTurnEnd;
/// When execution is interrupted.
pub struct OnAbort;

impl HookEventKind for OnPromptSubmit {
    type Input = PromptSubmitInfo;
    type Output = PromptAction;
}

impl HookEventKind for PreLlmRequest {
    type Input = PreRequestInfo;
    type Output = PreRequestAction;
}

impl HookEventKind for PreToolCall {
    type Input = ToolCallSummary;
    type Output = PreToolAction;
}

impl HookEventKind for PostToolCall {
    type Input = ToolResultSummary;
    type Output = PostToolAction;
}

impl HookEventKind for OnTurnEnd {
    type Input = TurnEndInfo;
    type Output = TurnEndAction;
}

impl HookEventKind for OnAbort {
    type Input = AbortInfo;
    type Output = ();
}

// =============================================================================
// Hook Trait
// =============================================================================

/// Async hook for a specific event kind.
///
/// Hooks receive a shared reference to the event's read-only input
/// and return a control-flow action. Multiple hooks can be registered
/// per event; they are evaluated in registration order and
/// short-circuit on the first non-Continue (or non-Finish) result.
#[async_trait]
pub trait Hook<E: HookEventKind>: Send + Sync {
    async fn call(&self, input: &E::Input) -> E::Output;
}

// =============================================================================
// Hook Registry
// =============================================================================

/// Builder for constructing a frozen `HookRegistry`.
///
/// Hooks are added during setup, then `build()` produces an immutable
/// registry that can be shared via `Arc`.
#[derive(Default)]
pub struct HookRegistryBuilder {
    on_prompt_submit: Vec<Box<dyn Hook<OnPromptSubmit>>>,
    pre_llm_request: Vec<Box<dyn Hook<PreLlmRequest>>>,
    pre_tool_call: Vec<Box<dyn Hook<PreToolCall>>>,
    post_tool_call: Vec<Box<dyn Hook<PostToolCall>>>,
    on_turn_end: Vec<Box<dyn Hook<OnTurnEnd>>>,
    on_abort: Vec<Box<dyn Hook<OnAbort>>>,
}

impl HookRegistryBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_on_prompt_submit(&mut self, hook: impl Hook<OnPromptSubmit> + 'static) {
        self.on_prompt_submit.push(Box::new(hook));
    }

    pub fn add_pre_llm_request(&mut self, hook: impl Hook<PreLlmRequest> + 'static) {
        self.pre_llm_request.push(Box::new(hook));
    }

    pub fn add_pre_tool_call(&mut self, hook: impl Hook<PreToolCall> + 'static) {
        self.pre_tool_call.push(Box::new(hook));
    }

    pub fn add_post_tool_call(&mut self, hook: impl Hook<PostToolCall> + 'static) {
        self.post_tool_call.push(Box::new(hook));
    }

    pub fn add_on_turn_end(&mut self, hook: impl Hook<OnTurnEnd> + 'static) {
        self.on_turn_end.push(Box::new(hook));
    }

    pub fn add_on_abort(&mut self, hook: impl Hook<OnAbort> + 'static) {
        self.on_abort.push(Box::new(hook));
    }

    /// Freeze the builder into an immutable registry.
    pub fn build(self) -> HookRegistry {
        HookRegistry {
            on_prompt_submit: self.on_prompt_submit,
            pre_llm_request: self.pre_llm_request,
            pre_tool_call: self.pre_tool_call,
            post_tool_call: self.post_tool_call,
            on_turn_end: self.on_turn_end,
            on_abort: self.on_abort,
        }
    }
}

/// Frozen registry of hooks. Constructed via [`HookRegistryBuilder::build()`].
pub struct HookRegistry {
    pub(crate) on_prompt_submit: Vec<Box<dyn Hook<OnPromptSubmit>>>,
    pub(crate) pre_llm_request: Vec<Box<dyn Hook<PreLlmRequest>>>,
    pub(crate) pre_tool_call: Vec<Box<dyn Hook<PreToolCall>>>,
    pub(crate) post_tool_call: Vec<Box<dyn Hook<PostToolCall>>>,
    pub(crate) on_turn_end: Vec<Box<dyn Hook<OnTurnEnd>>>,
    pub(crate) on_abort: Vec<Box<dyn Hook<OnAbort>>>,
}
