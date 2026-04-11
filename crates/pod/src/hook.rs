//! Pod-layer hook infrastructure
//!
//! Provides the `Hook<E>` trait and `HookRegistry` for orchestration hooks
//! that govern control-flow decisions in the Worker execution loop.
//!
//! The type system (`HookEventKind` / `Hook<E>`) mirrors the pattern
//! originally in llm-worker, now at the insomnia layer where orchestration
//! concerns belong.

use async_trait::async_trait;
use llm_worker::interceptor::{
    PostToolAction, PreRequestAction, PreToolAction, PromptAction, ToolCallInfo, ToolResultInfo,
    TurnEndAction,
};
use llm_worker::Item;

// =============================================================================
// Hook Event Kinds
// =============================================================================

/// Marker trait for hook event kinds.
///
/// Each event kind specifies its input (passed mutably to hooks) and
/// output (the control-flow action returned by hooks).
pub trait HookEventKind: Send + Sync + 'static {
    /// Mutable input passed to the hook.
    type Input;
    /// Control-flow action returned by the hook.
    type Output;
}

// --- Event kind markers ---

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
    type Input = Item;
    type Output = PromptAction;
}

impl HookEventKind for PreLlmRequest {
    type Input = Vec<Item>;
    type Output = PreRequestAction;
}

impl HookEventKind for PreToolCall {
    type Input = ToolCallInfo;
    type Output = PreToolAction;
}

impl HookEventKind for PostToolCall {
    type Input = ToolResultInfo;
    type Output = PostToolAction;
}

impl HookEventKind for OnTurnEnd {
    type Input = Vec<Item>;
    type Output = TurnEndAction;
}

impl HookEventKind for OnAbort {
    type Input = String;
    type Output = ();
}

// =============================================================================
// Hook Trait
// =============================================================================

/// Async hook for a specific event kind.
///
/// Hooks receive mutable access to the event's input and return a
/// control-flow action. Multiple hooks can be registered per event;
/// they are evaluated in registration order and short-circuit on the
/// first non-Continue result.
#[async_trait]
pub trait Hook<E: HookEventKind>: Send + Sync {
    async fn call(&self, input: &mut E::Input) -> E::Output;
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
