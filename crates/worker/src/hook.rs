//! Worker-layer hook infrastructure
//!
//! Hooks are the **public** orchestration extension point. They receive
//! event-specific context values about each event in the Engine execution loop
//! and return a safe public control-flow action. Contexts may carry narrow
//! host-created handles for approved side effects; hook return values remain
//! flow-control decisions only.
//!
//! Hooks intentionally cannot mutate the Engine's context, history, tool
//! call, or tool result. Internal mechanisms that need such access (e.g.
//! compaction, notification injection, output truncation) implement
//! `agen::Interceptor` directly inside Worker, never via this trait.
//!
//! This separation lets Hooks be exposed safely to user-facing
//! extension surfaces (scripting, plugins) in the future without
//! exposing the underlying mutable state.

use std::ops::Deref;
use std::sync::{Arc, Mutex};

use agen::HistoryEntry;
use agen::interceptor::{
    PostToolAction, PreRequestAction, PreToolAction, PromptAction, TurnEndAction,
};
use agen::tool::{ToolOutput, ToolResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use session_store::{SystemItem, SystemReminder};
use thiserror::Error;

use crate::session_history::SessionHistoryMetadata;

const HOOK_DIAGNOSTIC_MAX_BYTES: usize = 1_024;

/// Failure category exposed by the safe Worker hook boundary.
///
/// Categories are intentionally closed and payload-free so extensions cannot
/// smuggle provider, credential, or history data into diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookErrorCategory {
    InvalidInput,
    Dependency,
    Timeout,
    Cancelled,
    Trap,
    ScopeDisposed,
    Internal,
}

/// Bounded hook callback failure. Raw tool arguments, output, prompts, and
/// credentials must never be placed in `diagnostic`.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[error("{category:?}: {diagnostic}")]
pub struct HookError {
    pub category: HookErrorCategory,
    pub diagnostic: String,
}

impl HookError {
    pub fn new(category: HookErrorCategory, diagnostic: impl Into<String>) -> Self {
        Self {
            category,
            diagnostic: bounded_utf8(diagnostic.into(), HOOK_DIAGNOSTIC_MAX_BYTES),
        }
    }
}

/// Failure behavior declared when a hook is registered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookFailurePolicy {
    /// Gate the current operation when the hook cannot decide safely.
    FailClosed,
    /// Keep the already-authorized operation moving and emit a diagnostic.
    FailOpenWithDiagnostic,
    /// Keep committed state intact and mark the failure for operator attention.
    AttentionRequired,
}

fn bounded_utf8(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value
}

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
/// string that Worker converts into a synthetic tool result for the current
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
/// Hook code can use this handle only when the Worker host includes it in an
/// event-specific context. The handle queues typed requests; the host drains the
/// queue, commits each entry through `LogEntry::AnnotatedSystemItem`, and only then makes
/// the matching system message visible to the model. It deliberately exposes no
/// raw `agen::Item`, history writer, event sender, `Worker`, `Engine`, or
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
    /// The body is committed verbatim as the typed item's system-message text.
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
    /// Number of items currently in the Engine context.
    pub item_count: usize,
    /// Most recently observed `input_tokens` from the LLM provider.
    /// `None` when the Worker has no compaction state attached, or when
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
/// After every terminal assistant response is committed; observational except it may pause.
pub struct OnTurnEnd;

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
    async fn call(&self, input: &E::Input) -> Result<E::Output, HookError>;
}

// =============================================================================
// Hook Registry
// =============================================================================

/// Stable provenance attached to every Worker lifecycle callback.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HookInvocationContext {
    pub workspace_id: Option<String>,
    pub worker_id: String,
    pub session_id: String,
    pub session_revision: u64,
    pub run_id: Option<String>,
    pub turn_index: Option<usize>,
    pub call_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunCommittedExit {
    Finished,
    Paused,
    Yielded,
    Interrupted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunExitContext {
    pub invocation: HookInvocationContext,
    pub exit: RunCommittedExit,
    pub history_len: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunCommittedContext {
    pub invocation: HookInvocationContext,
    pub exit: RunCommittedExit,
    pub committed_history: Vec<HistoryEntry<SessionHistoryMetadata>>,
    pub committed_history_len: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionRewriteKind {
    Rewind,
    Compact,
    Fork,
    Restore,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BeforeSessionRewriteContext {
    pub invocation: HookInvocationContext,
    pub kind: SessionRewriteKind,
    pub current_history: Vec<HistoryEntry<SessionHistoryMetadata>>,
    pub current_history_len: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BeforeSessionRewriteAction {
    Continue,
    Deny(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerStoppingContext {
    pub invocation: HookInvocationContext,
    pub reason: String,
}

pub struct RunExit;
pub struct RunCommitted;
pub struct BeforeSessionRewrite;
pub struct WorkerStopping;

impl HookEventKind for RunExit {
    type Input = RunExitContext;
    type Output = ();
}

impl HookEventKind for RunCommitted {
    type Input = RunCommittedContext;
    type Output = ();
}

impl HookEventKind for BeforeSessionRewrite {
    type Input = BeforeSessionRewriteContext;
    type Output = BeforeSessionRewriteAction;
}

impl HookEventKind for WorkerStopping {
    type Input = WorkerStoppingContext;
    type Output = ();
}

pub(crate) struct RegisteredHook<E: HookEventKind> {
    owner: String,
    policy: HookFailurePolicy,
    hook: Box<dyn Hook<E>>,
}

impl<E: HookEventKind> RegisteredHook<E> {
    pub(crate) async fn call(&self, input: &E::Input) -> Result<E::Output, HookExecutionError> {
        self.hook
            .call(input)
            .await
            .map_err(|source| HookExecutionError {
                owner: self.owner.clone(),
                policy: self.policy,
                source,
            })
    }

    pub(crate) async fn call_optional(
        &self,
        input: &E::Input,
    ) -> Result<Option<E::Output>, HookExecutionError> {
        match self.call(input).await {
            Ok(output) => Ok(Some(output)),
            Err(error) if error.policy == HookFailurePolicy::FailOpenWithDiagnostic => {
                tracing::warn!(owner = %error.owner, error = %error.source, "inline hook failed open");
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[error("hook `{owner}` failed under {policy:?}: {source}")]
pub struct HookExecutionError {
    pub owner: String,
    pub policy: HookFailurePolicy,
    pub source: HookError,
}

/// Builder for constructing a frozen `HookRegistry`.
#[derive(Default)]
pub struct HookRegistryBuilder {
    on_prompt_submit: Vec<RegisteredHook<OnPromptSubmit>>,
    pre_llm_request: Vec<RegisteredHook<PreLlmRequest>>,
    pre_tool_call: Vec<RegisteredHook<PreToolCall>>,
    post_tool_call: Vec<RegisteredHook<PostToolCall>>,
    on_turn_end: Vec<RegisteredHook<OnTurnEnd>>,
    run_exit: Vec<RegisteredHook<RunExit>>,
    run_committed: Vec<RegisteredHook<RunCommitted>>,
    before_session_rewrite: Vec<RegisteredHook<BeforeSessionRewrite>>,
    worker_stopping: Vec<RegisteredHook<WorkerStopping>>,
}

macro_rules! add_hook_methods {
    ($default:ident, $named:ident, $field:ident, $event:ty) => {
        pub fn $default(&mut self, hook: impl Hook<$event> + 'static) {
            self.$named("worker.host", HookFailurePolicy::FailClosed, hook);
        }

        pub fn $named(
            &mut self,
            owner: impl Into<String>,
            policy: HookFailurePolicy,
            hook: impl Hook<$event> + 'static,
        ) {
            self.$field.push(RegisteredHook {
                owner: owner.into(),
                policy,
                hook: Box::new(hook),
            });
        }
    };
}

impl HookRegistryBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    add_hook_methods!(
        add_on_prompt_submit,
        add_named_on_prompt_submit,
        on_prompt_submit,
        OnPromptSubmit
    );
    add_hook_methods!(
        add_pre_llm_request,
        add_named_pre_llm_request,
        pre_llm_request,
        PreLlmRequest
    );
    add_hook_methods!(
        add_pre_tool_call,
        add_named_pre_tool_call,
        pre_tool_call,
        PreToolCall
    );
    add_hook_methods!(
        add_post_tool_call,
        add_named_post_tool_call,
        post_tool_call,
        PostToolCall
    );
    add_hook_methods!(
        add_on_turn_end,
        add_named_on_turn_end,
        on_turn_end,
        OnTurnEnd
    );
    add_hook_methods!(add_run_exit, add_named_run_exit, run_exit, RunExit);
    add_hook_methods!(
        add_run_committed,
        add_named_run_committed,
        run_committed,
        RunCommitted
    );
    add_hook_methods!(
        add_before_session_rewrite,
        add_named_before_session_rewrite,
        before_session_rewrite,
        BeforeSessionRewrite
    );
    add_hook_methods!(
        add_worker_stopping,
        add_named_worker_stopping,
        worker_stopping,
        WorkerStopping
    );

    pub(crate) fn checkpoint(&self) -> [usize; 9] {
        [
            self.on_prompt_submit.len(),
            self.pre_llm_request.len(),
            self.pre_tool_call.len(),
            self.post_tool_call.len(),
            self.on_turn_end.len(),
            self.run_exit.len(),
            self.run_committed.len(),
            self.before_session_rewrite.len(),
            self.worker_stopping.len(),
        ]
    }

    pub(crate) fn rollback_to(&mut self, checkpoint: [usize; 9]) {
        self.on_prompt_submit.truncate(checkpoint[0]);
        self.pre_llm_request.truncate(checkpoint[1]);
        self.pre_tool_call.truncate(checkpoint[2]);
        self.post_tool_call.truncate(checkpoint[3]);
        self.on_turn_end.truncate(checkpoint[4]);
        self.run_exit.truncate(checkpoint[5]);
        self.run_committed.truncate(checkpoint[6]);
        self.before_session_rewrite.truncate(checkpoint[7]);
        self.worker_stopping.truncate(checkpoint[8]);
    }

    pub fn build(self) -> HookRegistry {
        HookRegistry {
            on_prompt_submit: self.on_prompt_submit,
            pre_llm_request: self.pre_llm_request,
            pre_tool_call: self.pre_tool_call,
            post_tool_call: self.post_tool_call,
            on_turn_end: self.on_turn_end,
            run_exit: self.run_exit,
            run_committed: self.run_committed,
            before_session_rewrite: self.before_session_rewrite,
            worker_stopping: self.worker_stopping,
            diagnostics: std::sync::Mutex::new(Vec::new()),
        }
    }
}

/// Frozen registry of hooks. Constructed via [`HookRegistryBuilder::build()`].
pub struct HookRegistry {
    pub(crate) on_prompt_submit: Vec<RegisteredHook<OnPromptSubmit>>,
    pub(crate) pre_llm_request: Vec<RegisteredHook<PreLlmRequest>>,
    pub(crate) pre_tool_call: Vec<RegisteredHook<PreToolCall>>,
    pub(crate) post_tool_call: Vec<RegisteredHook<PostToolCall>>,
    pub(crate) on_turn_end: Vec<RegisteredHook<OnTurnEnd>>,
    run_exit: Vec<RegisteredHook<RunExit>>,
    run_committed: Vec<RegisteredHook<RunCommitted>>,
    before_session_rewrite: Vec<RegisteredHook<BeforeSessionRewrite>>,
    worker_stopping: Vec<RegisteredHook<WorkerStopping>>,
    diagnostics: std::sync::Mutex<Vec<HookExecutionError>>,
}

impl HookRegistry {
    fn record_diagnostic(&self, error: HookExecutionError) {
        let mut diagnostics = self.diagnostics.lock().expect("hook diagnostics poisoned");
        diagnostics.push(error);
        if diagnostics.len() > 128 {
            let remove = diagnostics.len() - 128;
            diagnostics.drain(..remove);
        }
    }

    pub fn diagnostics(&self) -> Vec<HookExecutionError> {
        self.diagnostics
            .lock()
            .expect("hook diagnostics poisoned")
            .clone()
    }
    pub async fn on_run_exit(&self, context: &RunExitContext) -> Result<(), HookExecutionError> {
        for registration in &self.run_exit {
            if let Err(error) = registration.call(context).await {
                self.record_diagnostic(error.clone());
                match error.policy {
                    HookFailurePolicy::FailOpenWithDiagnostic => {
                        tracing::warn!(owner = %error.owner, error = %error.source, "run-exit hook failed open");
                    }
                    HookFailurePolicy::FailClosed | HookFailurePolicy::AttentionRequired => {
                        return Err(error);
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn on_run_committed(
        &self,
        context: &RunCommittedContext,
    ) -> Result<(), HookExecutionError> {
        for registration in &self.run_committed {
            if let Err(error) = registration.call(context).await {
                self.record_diagnostic(error.clone());
                match error.policy {
                    HookFailurePolicy::FailOpenWithDiagnostic => {
                        tracing::warn!(owner = %error.owner, error = %error.source, "run-committed hook failed open");
                    }
                    HookFailurePolicy::FailClosed | HookFailurePolicy::AttentionRequired => {
                        return Err(error);
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn before_session_rewrite(
        &self,
        context: &BeforeSessionRewriteContext,
    ) -> Result<BeforeSessionRewriteAction, HookExecutionError> {
        let mut denials = Vec::new();
        for registration in &self.before_session_rewrite {
            match registration.call(context).await {
                Ok(BeforeSessionRewriteAction::Continue) => {}
                Ok(BeforeSessionRewriteAction::Deny(reason)) => {
                    denials.push((registration.owner.clone(), reason));
                }
                Err(error) if error.policy == HookFailurePolicy::FailOpenWithDiagnostic => {
                    self.record_diagnostic(error.clone());
                    tracing::warn!(owner = %error.owner, error = %error.source, "session-rewrite hook failed open");
                }
                Err(error) => {
                    self.record_diagnostic(error.clone());
                    return Err(error);
                }
            }
        }
        denials.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(denials
            .into_iter()
            .next()
            .map(|(_, reason)| BeforeSessionRewriteAction::Deny(reason))
            .unwrap_or(BeforeSessionRewriteAction::Continue))
    }

    pub async fn on_worker_stopping(
        &self,
        context: &WorkerStoppingContext,
    ) -> Result<(), HookExecutionError> {
        for registration in &self.worker_stopping {
            if let Err(error) = registration.call(context).await {
                self.record_diagnostic(error.clone());
                match error.policy {
                    HookFailurePolicy::FailOpenWithDiagnostic
                    | HookFailurePolicy::AttentionRequired => {
                        tracing::warn!(owner = %error.owner, error = %error.source, "worker-stopping hook requires attention");
                    }
                    HookFailurePolicy::FailClosed => return Err(error),
                }
            }
        }
        Ok(())
    }
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
                assert_eq!(body, "remember tasks");
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

    struct RewriteHook {
        action: BeforeSessionRewriteAction,
    }

    #[async_trait]
    impl Hook<BeforeSessionRewrite> for RewriteHook {
        async fn call(
            &self,
            _input: &BeforeSessionRewriteContext,
        ) -> Result<BeforeSessionRewriteAction, HookError> {
            Ok(self.action.clone())
        }
    }

    struct FailingRewriteHook;

    #[async_trait]
    impl Hook<BeforeSessionRewrite> for FailingRewriteHook {
        async fn call(
            &self,
            _input: &BeforeSessionRewriteContext,
        ) -> Result<BeforeSessionRewriteAction, HookError> {
            Err(HookError::new(
                HookErrorCategory::Dependency,
                "provider unavailable",
            ))
        }
    }

    fn rewrite_context() -> BeforeSessionRewriteContext {
        BeforeSessionRewriteContext {
            invocation: HookInvocationContext {
                workspace_id: Some("workspace".into()),
                worker_id: "worker".into(),
                session_id: "session".into(),
                session_revision: 4,
                run_id: None,
                turn_index: None,
                call_id: None,
            },
            kind: SessionRewriteKind::Compact,
            current_history: Vec::new(),
            current_history_len: 8,
        }
    }

    #[tokio::test]
    async fn rewrite_denials_are_resolved_by_owner_not_registration_order() {
        let mut builder = HookRegistryBuilder::new();
        builder.add_named_before_session_rewrite(
            "z-feature",
            HookFailurePolicy::FailClosed,
            RewriteHook {
                action: BeforeSessionRewriteAction::Deny("z denied".into()),
            },
        );
        builder.add_named_before_session_rewrite(
            "a-feature",
            HookFailurePolicy::FailClosed,
            RewriteHook {
                action: BeforeSessionRewriteAction::Deny("a denied".into()),
            },
        );

        assert_eq!(
            builder
                .build()
                .before_session_rewrite(&rewrite_context())
                .await
                .unwrap(),
            BeforeSessionRewriteAction::Deny("a denied".into())
        );
    }

    #[tokio::test]
    async fn hook_failure_policy_is_applied_at_the_registry_boundary() {
        let mut fail_open = HookRegistryBuilder::new();
        fail_open.add_named_before_session_rewrite(
            "feature",
            HookFailurePolicy::FailOpenWithDiagnostic,
            FailingRewriteHook,
        );
        let fail_open = fail_open.build();
        assert_eq!(
            fail_open
                .before_session_rewrite(&rewrite_context())
                .await
                .unwrap(),
            BeforeSessionRewriteAction::Continue
        );
        assert_eq!(fail_open.diagnostics().len(), 1);

        let mut fail_closed = HookRegistryBuilder::new();
        fail_closed.add_named_before_session_rewrite(
            "feature",
            HookFailurePolicy::FailClosed,
            FailingRewriteHook,
        );
        let error = fail_closed
            .build()
            .before_session_rewrite(&rewrite_context())
            .await
            .unwrap_err();
        assert_eq!(error.source.category, HookErrorCategory::Dependency);
    }

    #[test]
    fn hook_diagnostics_are_utf8_bounded() {
        let error = HookError::new(HookErrorCategory::Internal, "界".repeat(1_000));
        assert!(error.diagnostic.len() <= HOOK_DIAGNOSTIC_MAX_BYTES);
        assert!(error.diagnostic.is_char_boundary(error.diagnostic.len()));
    }
}
