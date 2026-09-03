//! Interceptor - control flow delegation for the Engine execution loop
//!
//! Defines the [`Interceptor`] trait that callers implement to inject
//! orchestration decisions (approval, skip, pause, abort) into the Engine's
//! turn loop without the Engine knowing about host-application concepts.

use std::sync::Arc;

use async_trait::async_trait;

use crate::Item;
use crate::tool::{Tool, ToolCall, ToolExecutionContext, ToolMeta, ToolResult};

// =============================================================================
// Failure Types
// =============================================================================

/// A typed failure returned by an [`Interceptor`] implementation.
///
/// The Engine attaches the exact [`InterceptorPoint`] at which the failure was
/// observed before exposing it through the run termination boundary.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct InterceptorError {
    message: String,
}

impl InterceptorError {
    /// Create an interceptor failure with a caller-defined message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Return the failure message supplied by the interceptor.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl From<String> for InterceptorError {
    fn from(message: String) -> Self {
        Self::new(message)
    }
}

impl From<&str> for InterceptorError {
    fn from(message: &str) -> Self {
        Self::new(message)
    }
}

/// The Engine lifecycle point at which an interceptor failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterceptorPoint {
    PromptSubmit,
    PendingHistoryAppends,
    PreLlmRequest,
    PreToolCall,
    PostToolCall,
    TurnEnd,
    Abort,
}

impl std::fmt::Display for InterceptorPoint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::PromptSubmit => "prompt_submit",
            Self::PendingHistoryAppends => "pending_history_appends",
            Self::PreLlmRequest => "pre_llm_request",
            Self::PreToolCall => "pre_tool_call",
            Self::PostToolCall => "post_tool_call",
            Self::TurnEnd => "turn_end",
            Self::Abort => "abort",
        };
        formatter.write_str(name)
    }
}

/// An interceptor failure bound to the exact Engine lifecycle point that ran it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{point} interceptor failed: {error}")]
pub struct InterceptorFailure {
    point: InterceptorPoint,
    #[source]
    error: InterceptorError,
}

impl InterceptorFailure {
    pub(crate) fn new(point: InterceptorPoint, error: InterceptorError) -> Self {
        Self { point, error }
    }

    /// The lifecycle point that returned the failure.
    pub fn point(&self) -> InterceptorPoint {
        self.point
    }

    /// The typed error returned by the interceptor.
    pub fn error(&self) -> &InterceptorError {
        &self.error
    }
}

/// Result returned by asynchronous interceptor lifecycle methods.
pub type InterceptorResult<T> = Result<T, InterceptorError>;

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
    /// Turn is finished, return to caller.
    Finish,
    /// Continue with additional messages injected into history.
    ContinueWithMessages(Vec<Item>),
    /// Pause execution (can be resumed later).
    Pause,
}

// =============================================================================
// Context Types
// =============================================================================

/// Context for pre-tool-call decisions.
pub struct ToolCallInfo {
    /// Tool call information (modifiable).
    pub call: ToolCall,
    /// Tool meta information.
    pub meta: ToolMeta,
    /// Tool instance (for state access).
    pub tool: Arc<dyn Tool>,
    /// Response-local execution context for this call.
    pub context: ToolExecutionContext,
}

/// Context for post-tool-call decisions.
pub struct ToolResultInfo {
    /// Original tool call.
    pub call: ToolCall,
    /// Tool execution result (modifiable).
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
/// with the exact [`InterceptorPoint`] that failed.
///
/// All methods have default implementations that let the Engine proceed
/// without intervention. Callers provide richer implementations for approval
/// flows, permission checks, and other trusted host adaptation.
#[async_trait]
pub trait Interceptor: Send + Sync {
    /// Called after receiving user input, before adding it to Engine history.
    async fn on_prompt_submit(&self, _item: &mut Item) -> InterceptorResult<PromptAction> {
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
    async fn pending_history_appends(&self) -> InterceptorResult<Vec<Item>> {
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
        _context: &mut Vec<Item>,
    ) -> InterceptorResult<PreRequestAction> {
        Ok(PreRequestAction::Continue)
    }

    /// Called before each tool is executed.
    async fn pre_tool_call(&self, _info: &mut ToolCallInfo) -> InterceptorResult<PreToolAction> {
        Ok(PreToolAction::Continue)
    }

    /// Called after each tool reaches one terminal result.
    async fn post_tool_call(
        &self,
        _info: &mut ToolResultInfo,
    ) -> InterceptorResult<PostToolAction> {
        Ok(PostToolAction::Continue)
    }

    /// Called at the assistant boundary when a completed response has no tool calls.
    ///
    /// This is not the logical run termination observer. A host that needs that
    /// boundary must inspect the returned [`crate::EngineRunExit`].
    async fn on_turn_end(&self, _history: &[Item]) -> InterceptorResult<TurnEndAction> {
        Ok(TurnEndAction::Finish)
    }

    /// Called once when execution is interrupted (abort, cancellation, or failure).
    async fn on_abort(&self, _reason: &str) -> InterceptorResult<()> {
        Ok(())
    }
}

/// Default interceptor: no intervention. Engine proceeds through the loop
/// without any external control flow decisions.
pub(crate) struct DefaultInterceptor;

#[async_trait]
impl Interceptor for DefaultInterceptor {}
