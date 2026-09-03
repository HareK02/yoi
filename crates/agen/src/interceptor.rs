//! Interceptor - control flow delegation for the Engine execution loop
//!
//! Defines the [`Interceptor`] trait that callers implement to inject
//! orchestration decisions (approval, skip, pause, abort) into the Engine's
//! turn loop without the Engine knowing about host-application concepts.

use std::sync::Arc;

use async_trait::async_trait;

use crate::Item;
use crate::engine::EngineRunExit;
use crate::history::HistoryEntry;
use crate::tool::{Tool, ToolCall, ToolExecutionContext, ToolMeta, ToolResult};

// =============================================================================
// Typed lifecycle metadata and failures
// =============================================================================

/// Maximum UTF-8 byte length retained for interceptor diagnostics.
pub const MAX_INTERCEPTOR_DIAGNOSTIC_BYTES: usize = 1024;

/// Stable category for the source of an interceptor failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterceptorErrorCategory {
    Policy,
    Dependency,
    ContractViolation,
    Internal,
}

impl std::fmt::Display for InterceptorErrorCategory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Policy => "policy",
            Self::Dependency => "dependency",
            Self::ContractViolation => "contract_violation",
            Self::Internal => "internal",
        })
    }
}

/// A typed, bounded failure returned by an [`Interceptor`] implementation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{category}: {diagnostic}")]
pub struct InterceptorError {
    category: InterceptorErrorCategory,
    diagnostic: String,
}

impl InterceptorError {
    pub fn new(category: InterceptorErrorCategory, diagnostic: impl Into<String>) -> Self {
        let mut diagnostic = diagnostic.into();
        if diagnostic.len() > MAX_INTERCEPTOR_DIAGNOSTIC_BYTES {
            let mut end = MAX_INTERCEPTOR_DIAGNOSTIC_BYTES;
            while !diagnostic.is_char_boundary(end) {
                end -= 1;
            }
            diagnostic.truncate(end);
        }
        Self {
            category,
            diagnostic,
        }
    }

    pub fn category(&self) -> InterceptorErrorCategory {
        self.category
    }

    pub fn diagnostic(&self) -> &str {
        &self.diagnostic
    }
}

/// The lifecycle phase at which an interceptor callback executes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InterceptorPhase {
    #[default]
    PromptSubmit,
    PendingHistoryAppends,
    PreLlmRequest,
    PreToolCall,
    PostToolCall,
    AssistantTurnEnd,
    RunExit,
}

impl std::fmt::Display for InterceptorPhase {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::PromptSubmit => "prompt_submit",
            Self::PendingHistoryAppends => "pending_history_appends",
            Self::PreLlmRequest => "pre_llm_request",
            Self::PreToolCall => "pre_tool_call",
            Self::PostToolCall => "post_tool_call",
            Self::AssistantTurnEnd => "assistant_turn_end",
            Self::RunExit => "run_exit",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct InterceptorRunId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InterceptorTurnId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum InterceptorCallId {
    Llm(u64),
    Tool(String),
}

/// Saturating public counter used by interceptor contexts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct InterceptorCounter(u32);

impl InterceptorCounter {
    pub fn from_usize(value: usize) -> Self {
        Self(u32::try_from(value).unwrap_or(u32::MAX))
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InterceptorCounters {
    pub invocation: InterceptorCounter,
    pub engine_turn: InterceptorCounter,
    pub run_turn: InterceptorCounter,
    pub llm_call: InterceptorCounter,
    pub tool_batch: InterceptorCounter,
    pub tool_call: InterceptorCounter,
}

/// Identity, phase, and bounded counters common to every lifecycle callback.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InterceptorInvocation {
    pub run_id: InterceptorRunId,
    pub turn_id: Option<InterceptorTurnId>,
    pub call_id: Option<InterceptorCallId>,
    pub phase: InterceptorPhase,
    pub counters: InterceptorCounters,
}

/// An interceptor failure bound to the exact Engine lifecycle phase that ran it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{phase} interceptor failed: {error}")]
pub struct InterceptorFailure {
    phase: InterceptorPhase,
    #[source]
    error: InterceptorError,
}

impl InterceptorFailure {
    pub(crate) fn new(phase: InterceptorPhase, error: InterceptorError) -> Self {
        Self { phase, error }
    }

    pub fn phase(&self) -> InterceptorPhase {
        self.phase
    }

    pub fn error(&self) -> &InterceptorError {
        &self.error
    }
}

pub type InterceptorResult<T> = Result<T, InterceptorError>;

// =============================================================================
// Lifecycle Contexts
// =============================================================================

pub struct PromptSubmitContext<'a, A = ()> {
    pub invocation: InterceptorInvocation,
    pub item: &'a mut Item,
    pub history: &'a [HistoryEntry<A>],
}

pub struct PendingHistoryAppendsContext<'a, A = ()> {
    pub invocation: InterceptorInvocation,
    pub history: &'a [HistoryEntry<A>],
}

pub struct PreLlmRequestContext<'a, A = ()> {
    pub invocation: InterceptorInvocation,
    pub items: &'a mut Vec<Item>,
    pub history: &'a [HistoryEntry<A>],
}

pub struct AssistantTurnEndContext<'a, A = ()> {
    pub invocation: InterceptorInvocation,
    pub assistant_entries: &'a [HistoryEntry<A>],
    pub history: &'a [HistoryEntry<A>],
    pub tool_calls: &'a [ToolCall],
}

pub struct RunExitContext<'a, A = ()> {
    pub invocation: InterceptorInvocation,
    pub exit: &'a EngineRunExit,
    pub history: &'a [HistoryEntry<A>],
}

// =============================================================================
// Action Enums
// =============================================================================

/// Action after prompt submission.
#[derive(Debug, Clone, PartialEq)]
pub enum PromptAction {
    /// Proceed normally.
    Continue,
    /// Cancel with a reason.
    Cancel(String),
    /// Proceed, and append these items to history right after the user
    /// message. Mirrors [`TurnEndAction::ContinueWithMessages`] for the
    /// submit edge: lets the upper layer attach resolver-produced
    /// system messages (e.g. `@<path>` file content) so they sit
    /// adjacent to the user message that referenced them.
    ContinueWith(Vec<Item>),
}

/// Action before an LLM request.
#[derive(Debug, Clone, PartialEq)]
pub enum PreRequestAction {
    /// Proceed normally.
    Continue,
    /// Proceed after appending these items to durable engine history.
    ///
    /// This is for upper-layer budget/status nudges that the model may react
    /// to: the items are committed before the request so later turns can see
    /// why the engine changed course.
    ContinueWith(Vec<Item>),
    /// Yield after appending these items to durable engine history.
    ///
    /// This is for host-mediated pre-request appends that must be visible to
    /// usage accounting and compaction checks before the current LLM request is
    /// allowed to proceed.
    YieldWith(Vec<Item>),
    /// Cancel with a reason (treated as an error).
    Cancel(String),
    /// Yield control to the caller for external processing.
    ///
    /// The Engine exits the turn loop cleanly with `EngineResult::Yielded`.
    /// The caller is expected to resume execution later.
    Yield,
}

/// Action before a tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreToolAction {
    /// Proceed with execution.
    Continue,
    /// Skip this tool call (do not execute).
    Skip,
    /// Do not execute the tool call; commit this synthetic result instead.
    ///
    /// This preserves provider-visible `tool_use` / `tool_result` pairing
    /// without aborting the whole turn.
    SyntheticResult(ToolResult),
    /// Abort the entire run.
    Abort(String),
    /// Pause execution (can be resumed later).
    Pause,
}

/// Action after a tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostToolAction {
    /// Proceed normally.
    Continue,
    /// Abort the entire run.
    Abort(String),
}

/// Action at the end of a turn (when LLM produces no tool calls).
#[derive(Debug, Clone)]
pub enum TurnEndAction {
    /// Accept the Engine's natural next phase: execute tools, or finish when none exist.
    Finish,
    /// Commit additional messages, then continue through the natural next phase.
    ContinueWithMessages(Vec<Item>),
    /// Pause execution (can be resumed later).
    Pause,
}

// =============================================================================
// Context Types
// =============================================================================

/// Context for pre-tool-call decisions.
pub struct ToolCallInfo<'a, A = ()> {
    pub invocation: InterceptorInvocation,
    pub history: &'a [HistoryEntry<A>],
    pub call: ToolCall,
    /// Tool meta information.
    pub meta: ToolMeta,
    /// Tool instance (for state access).
    pub tool: Arc<dyn Tool>,
    /// Response-local execution context for this call.
    pub context: ToolExecutionContext,
}

/// Context for post-tool-call decisions.
pub struct ToolResultInfo<'a, A = ()> {
    pub invocation: InterceptorInvocation,
    pub history: &'a [HistoryEntry<A>],
    pub call: ToolCall,
    /// Committed terminal tool execution result.
    pub result: ToolResult,
    /// Tool meta information.
    pub meta: ToolMeta,
    /// Tool instance (for state access).
    pub tool: Arc<dyn Tool>,
    /// Response-local execution context for this call.
    pub context: ToolExecutionContext,
}

// =============================================================================
// Interceptor Trait
// =============================================================================

/// Intercepts the Engine execution loop at key decision points.
///
/// Every lifecycle method is asynchronous and returns [`InterceptorResult`],
/// keeping implementation failure separate from the method's control-flow
/// action. The Engine reports a failure as a typed run interruption annotated
/// with the exact [`InterceptorPhase`] that failed.
///
/// All methods have default implementations that let the Engine proceed
/// without intervention. Callers provide richer implementations for approval
/// flows, permission checks, and other trusted host adaptation.
#[async_trait]
pub trait Interceptor<A: Send + Sync = ()>: Send + Sync {
    /// Called after receiving user input, before adding it to Engine history.
    async fn on_prompt_submit(
        &self,
        _context: PromptSubmitContext<'_, A>,
    ) -> InterceptorResult<PromptAction> {
        Ok(PromptAction::Continue)
    }

    /// Items that should be **committed to `engine.history`** just
    /// before the next LLM request. Returned items are `extend`ed into
    /// the persistent history (and therefore picked up by the per-turn
    /// clone that backs the LLM request, plus the usual
    /// history-persistence path).
    ///
    /// Use this for inputs that arrive from outside the LLM and need
    /// to be reflected in the on-disk history — notifications,
    /// external events, system reminders. Do **not** use
    /// [`Self::pre_llm_request`] for that purpose: it mutates a
    /// per-request clone, so any committed assistant response that
    /// reacts to the injection would have no visible trigger on the
    /// next turn (or after resume / compaction).
    ///
    /// `pre_llm_request` remains the right place for purely
    /// reproducible per-request transformations (pruning, content
    /// trimming, cache anchors) that depend only on the existing
    /// history.
    async fn pending_history_appends(
        &self,
        _context: PendingHistoryAppendsContext<'_, A>,
    ) -> InterceptorResult<Vec<Item>> {
        Ok(Vec::new())
    }

    /// Called before each LLM request. The context starts as a clone
    /// of `engine.history` (after `pending_history_appends` and the
    /// Engine's own prune projection have been applied).
    ///
    /// Direct mutations to `context` remain request-local and are not persisted.
    /// If an interceptor derives a human/model-visible nudge from the current
    /// request context, return [`PreRequestAction::ContinueWith`] so the Engine
    /// commits it to history before the request is sent.
    async fn pre_llm_request(
        &self,
        _context: PreLlmRequestContext<'_, A>,
    ) -> InterceptorResult<PreRequestAction> {
        Ok(PreRequestAction::Continue)
    }

    /// Called before each tool is executed.
    async fn pre_tool_call(
        &self,
        _info: &mut ToolCallInfo<'_, A>,
    ) -> InterceptorResult<PreToolAction> {
        Ok(PreToolAction::Continue)
    }

    /// Called after each tool reaches one terminal result and that result is committed.
    async fn post_tool_call(
        &self,
        _info: &ToolResultInfo<'_, A>,
    ) -> InterceptorResult<PostToolAction> {
        Ok(PostToolAction::Continue)
    }

    /// Called after every terminal assistant response is committed and before
    /// the Engine decides whether to execute tools, continue, or finish.
    async fn on_assistant_turn_end(
        &self,
        _context: AssistantTurnEndContext<'_, A>,
    ) -> InterceptorResult<TurnEndAction> {
        Ok(TurnEndAction::Finish)
    }

    /// Called once for the terminal outcome of each public run or resume call.
    async fn on_run_exit(&self, _context: RunExitContext<'_, A>) -> InterceptorResult<()> {
        Ok(())
    }
}

/// Default interceptor: no intervention. Engine proceeds through the loop
/// without any external control flow decisions.
pub(crate) struct DefaultInterceptor;

#[async_trait]
impl<A: Send + Sync> Interceptor<A> for DefaultInterceptor {}
