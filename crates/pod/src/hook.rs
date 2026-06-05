//! Pod-layer hook infrastructure
//!
//! Hooks are the **public** orchestration extension point. They receive
//! event-specific context values about each event in the Worker execution loop
//! and return a safe public control-flow action. Contexts may carry narrow
//! host-created handles for approved side effects; hook return values remain
//! flow-control decisions only.
//!
//! Hooks intentionally cannot mutate the Worker's context, history, tool
//! call, or tool result. Internal mechanisms that need such access (e.g.
//! compaction, notification injection, output truncation) implement
//! `llm_worker::Interceptor` directly inside Pod, never via this trait.
//!
//! This separation lets Hooks be exposed safely to user-facing
//! extension surfaces (scripting, plugins) in the future without
//! exposing the underlying mutable state.

use std::ops::Deref;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use llm_worker::interceptor::{
    PostToolAction, PreRequestAction, PreToolAction, PromptAction, TurnEndAction,
};
use llm_worker::tool::{ToolOutput, ToolResult};
use serde_json::Value;
use session_store::{SystemItem, SystemReminder};

/// Hook-facing prompt-submit action.
///
/// A strict subset of [`PromptAction`]: Hooks may continue or cancel
/// the submit, but cannot inject items into history. The
/// `ContinueWith(Vec<Item>)` variant is reserved for the internal
/// `Interceptor` so that Hook (the public extension surface) stays
/// read-only by construction (see module-level doc).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookPromptAction {
    /// Proceed normally.
    Continue,
    /// Cancel this submitted prompt with a reason.
    Cancel(String),
}

impl From<HookPromptAction> for PromptAction {
    fn from(action: HookPromptAction) -> Self {
        match action {
            HookPromptAction::Continue => PromptAction::Continue,
            HookPromptAction::Cancel(reason) => PromptAction::Cancel(reason),
        }
    }
}

/// Hook-facing pre-LLM-request action.
///
/// Public hooks may observe the request boundary, cancel the run, or yield
/// control back to the caller. They cannot return
/// `PreRequestAction::ContinueWith(Vec<Item>)`; model-visible request/history
/// additions must use durable host-owned paths such as notifications or
/// system-item commits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookPreRequestAction {
    /// Proceed normally.
    Continue,
    /// Cancel the run with a reason.
    Cancel(String),
    /// Yield control to the caller for host-owned processing/resume.
    Yield,
}

impl From<HookPreRequestAction> for PreRequestAction {
    fn from(action: HookPreRequestAction) -> Self {
        match action {
            HookPreRequestAction::Continue => PreRequestAction::Continue,
            HookPreRequestAction::Cancel(reason) => PreRequestAction::Cancel(reason),
            HookPreRequestAction::Yield => PreRequestAction::Yield,
        }
    }
}

/// Hook-facing pre-tool-call action.
///
/// Hooks may continue, pause/abort the call, or deny it with an error
/// string that Pod converts into a synthetic tool result for the current
/// tool call. Hooks cannot express the internal no-result skip path, mutate
/// the tool call arguments, or construct arbitrary `ToolResult` values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookPreToolAction {
    /// Proceed with tool execution.
    Continue,
    /// Deny this tool call and commit a synthetic error result.
    Deny(String),
    /// Abort the entire run.
    Abort(String),
    /// Pause execution.
    Pause,
}

impl HookPreToolAction {
    pub(crate) fn into_worker_action(self, call_id: String) -> PreToolAction {
        match self {
            HookPreToolAction::Continue => PreToolAction::Continue,
            HookPreToolAction::Deny(reason) => {
                PreToolAction::SyntheticResult(ToolResult::error(call_id, reason))
            }
            HookPreToolAction::Abort(reason) => PreToolAction::Abort(reason),
            HookPreToolAction::Pause => PreToolAction::Pause,
        }
    }
}

/// Hook-facing post-tool-call action.
///
/// Post-tool hooks are observational except that they may abort the run. They
/// cannot rewrite the tool output; adding an explicit bounded transform would
/// require a separate safe public type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookPostToolAction {
    /// Proceed normally.
    Continue,
    /// Abort the entire run.
    Abort(String),
}

impl From<HookPostToolAction> for PostToolAction {
    fn from(action: HookPostToolAction) -> Self {
        match action {
            HookPostToolAction::Continue => PostToolAction::Continue,
            HookPostToolAction::Abort(reason) => PostToolAction::Abort(reason),
        }
    }
}

/// Hook-facing turn-end action.
///
/// Turn-end hooks may observe a completed turn and optionally pause further
/// execution. They cannot return
/// `TurnEndAction::ContinueWithMessages(Vec<Item>)`; public hooks must not
/// append arbitrary model-visible messages at turn boundaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookTurnEndAction {
    /// Finish the turn normally.
    Finish,
    /// Pause execution.
    Pause,
}

impl From<HookTurnEndAction> for TurnEndAction {
    fn from(action: HookTurnEndAction) -> Self {
        match action {
            HookTurnEndAction::Finish => TurnEndAction::Finish,
            HookTurnEndAction::Pause => TurnEndAction::Pause,
        }
    }
}

// =============================================================================
// Hook context handles
// =============================================================================

/// Host-created handle for appending approved durable [`SystemItem`] requests.
///
/// Hook code can use this handle only when the Pod host includes it in an
/// event-specific context. The handle queues typed requests; the host drains the
/// queue, commits each entry through `LogEntry::SystemItem`, and only then makes
/// the matching system message visible to the model. It deliberately exposes no
/// raw `llm_worker::Item`, history writer, event sender, `Pod`, `Worker`, or
/// notification buffer.
pub struct SystemItemAppendHandle {
    pending: Arc<Mutex<Vec<SystemItem>>>,
}

impl SystemItemAppendHandle {
    pub(crate) fn new(pending: Arc<Mutex<Vec<SystemItem>>>) -> Self {
        Self { pending }
    }

    /// Queue a task-inactivity reminder for durable model-visible append.
    ///
    /// The body should be the unwrapped reminder text; the host-side
    /// `SystemReminder` renderer wraps it exactly once in `<system-reminder>`
    /// tags before commit.
    pub fn append_task_reminder(&self, body: impl Into<String>) {
        let item = SystemReminder::task_inactivity(body).into_system_item();
        self.pending
            .lock()
            .expect("system-item append queue poisoned")
            .push(item);
    }
}

// =============================================================================
// Hook input summary/context types (read-only)
// =============================================================================

/// Information passed to `OnPromptSubmit` hooks.
pub struct PromptSubmitInfo {
    /// Concatenated text content of the user's input message.
    pub input_text: String,
    /// 0-based turn index this prompt opens.
    pub turn_index: usize,
}

/// Summary information included in `PreLlmRequest` contexts.
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

/// Context passed to `PreLlmRequest` hooks.
///
/// The summary remains read-only. When the host grants durable system-item
/// append authority for this request, `system_items()` exposes a typed append
/// handle; otherwise it returns `None` and hooks cannot produce model-visible
/// additions.
pub struct PreRequestContext {
    info: PreRequestInfo,
    system_items: Option<SystemItemAppendHandle>,
}

impl PreRequestContext {
    pub(crate) fn new(info: PreRequestInfo, system_items: Option<SystemItemAppendHandle>) -> Self {
        Self { info, system_items }
    }

    /// Read-only request summary.
    pub fn info(&self) -> &PreRequestInfo {
        &self.info
    }

    /// Host-provided durable system-item append handle, when available.
    pub fn system_items(&self) -> Option<&SystemItemAppendHandle> {
        self.system_items.as_ref()
    }
}

impl Deref for PreRequestContext {
    type Target = PreRequestInfo;

    fn deref(&self) -> &Self::Target {
        &self.info
    }
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
/// Each event kind specifies its read-only input and the safe public
/// control-flow action returned by hooks.
pub trait HookEventKind: Send + Sync + 'static {
    /// Read-only input passed to the hook.
    type Input: Send + Sync;
    /// Control-flow action returned by the hook.
    type Output;
}

/// After receiving user input, before adding to history; may continue or cancel.
pub struct OnPromptSubmit;
/// Before each LLM request; may continue, cancel, or yield.
pub struct PreLlmRequest;
/// Before each tool is executed; may continue, deny with a synthetic result,
/// abort, or pause.
pub struct PreToolCall;
/// After each tool completes; observational except it may abort the run.
pub struct PostToolCall;
/// When a turn ends with no tool calls; observational except it may pause.
pub struct OnTurnEnd;
/// When execution is interrupted; observational only.
pub struct OnAbort;

impl HookEventKind for OnPromptSubmit {
    type Input = PromptSubmitInfo;
    type Output = HookPromptAction;
}

impl HookEventKind for PreLlmRequest {
    type Input = PreRequestContext;
    type Output = HookPreRequestAction;
}

impl HookEventKind for PreToolCall {
    type Input = ToolCallSummary;
    type Output = HookPreToolAction;
}

impl HookEventKind for PostToolCall {
    type Input = ToolResultSummary;
    type Output = HookPostToolAction;
}

impl HookEventKind for OnTurnEnd {
    type Input = TurnEndInfo;
    type Output = HookTurnEndAction;
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
/// and return a safe public control-flow action. Multiple hooks can be
/// registered per event; they are evaluated in registration order and
/// short-circuit on the first non-continue action.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_item_append_handle_queues_only_approved_task_reminder_items() {
        let pending = Arc::new(Mutex::new(Vec::new()));
        let handle = SystemItemAppendHandle::new(Arc::clone(&pending));

        handle.append_task_reminder("remember tasks");

        let queued = pending.lock().expect("pending queue poisoned");
        assert_eq!(queued.len(), 1);
        match &queued[0] {
            SystemItem::TaskReminder { body, .. } => {
                assert_eq!(body.matches("<system-reminder>").count(), 1);
                assert!(body.contains("remember tasks"));
            }
            other => panic!("unexpected system item: {other:?}"),
        }
    }

    #[test]
    fn pre_request_context_exposes_handle_only_when_host_supplies_one() {
        let info = PreRequestInfo {
            item_count: 3,
            estimated_tokens: Some(42),
            turn_index: 1,
            tool_calls_this_turn: 2,
        };
        let context = PreRequestContext::new(info, None);

        assert_eq!(context.item_count, 3);
        assert_eq!(context.info().estimated_tokens, Some(42));
        assert!(context.system_items().is_none());
    }

    #[test]
    fn public_pre_tool_hook_actions_cannot_emit_internal_no_result_skip() {
        let continue_action = HookPreToolAction::Continue.into_worker_action("call_1".into());
        assert!(matches!(continue_action, PreToolAction::Continue));

        let deny_action =
            HookPreToolAction::Deny("blocked".into()).into_worker_action("call_2".into());
        match deny_action {
            PreToolAction::SyntheticResult(result) => {
                assert_eq!(result.tool_use_id, "call_2");
                assert_eq!(result.summary, "blocked");
                assert!(result.is_error);
            }
            other => panic!("public deny must produce synthetic result, got {other:?}"),
        }

        let abort_action =
            HookPreToolAction::Abort("stop".into()).into_worker_action("call_3".into());
        assert!(matches!(abort_action, PreToolAction::Abort(reason) if reason == "stop"));

        let pause_action = HookPreToolAction::Pause.into_worker_action("call_4".into());
        assert!(matches!(pause_action, PreToolAction::Pause));
    }
}
