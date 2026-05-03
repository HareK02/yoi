//! Interceptor - control flow delegation for the Worker execution loop
//!
//! Defines the [`Interceptor`] trait that upper layers (e.g. Pod) implement
//! to inject orchestration decisions (approval, skip, pause, abort)
//! into the Worker's turn loop without the Worker knowing about
//! higher-level concepts.

use std::sync::Arc;

use async_trait::async_trait;

use crate::Item;
use crate::tool::{Tool, ToolCall, ToolMeta, ToolResult};

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreRequestAction {
    /// Proceed normally.
    Continue,
    /// Cancel with a reason (treated as an error).
    Cancel(String),
    /// Yield control to the caller for external processing.
    ///
    /// The Worker exits the turn loop cleanly with `WorkerResult::Yielded`.
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
}

// =============================================================================
// Interceptor Trait
// =============================================================================

/// Intercepts the Worker execution loop at key decision points.
///
/// All methods have default implementations that let the Worker
/// proceed without intervention. Upper layers (e.g. Pod) provide
/// richer implementations for approval flows, permission checks, etc.
#[async_trait]
pub trait Interceptor: Send + Sync {
    /// Called after receiving user input, before adding to history.
    async fn on_prompt_submit(&self, _item: &mut Item) -> PromptAction {
        PromptAction::Continue
    }

    /// Items that should be **committed to `worker.history`** just
    /// before the next LLM request. Returned items are `extend`ed into
    /// the persistent history (and therefore picked up by the per-turn
    /// clone that backs the LLM request, plus the usual
    /// history-persistence path).
    ///
    /// Use this for inputs that arrive from outside the LLM and need
    /// to be reflected in the on-disk history — notifications,
    /// cross-Pod events, system reminders. Do **not** use
    /// [`Self::pre_llm_request`] for that purpose: it mutates a
    /// per-request clone, so any committed assistant response that
    /// reacts to the injection would have no visible trigger on the
    /// next turn (or after resume / compaction).
    ///
    /// `pre_llm_request` remains the right place for purely
    /// reproducible per-request transformations (pruning, content
    /// trimming, cache anchors) that depend only on the existing
    /// history.
    async fn pending_history_appends(&self) -> Vec<Item> {
        Vec::new()
    }

    /// Called before each LLM request. The context starts as a clone
    /// of `worker.history` (after `pending_history_appends` and the
    /// Worker's own prune projection have been applied) and can be
    /// further modified for that single request only — mutations here
    /// are **not** persisted back to history. Use
    /// [`Self::pending_history_appends`] for inputs that need to land
    /// in history.
    async fn pre_llm_request(&self, _context: &mut Vec<Item>) -> PreRequestAction {
        PreRequestAction::Continue
    }

    /// Called before each tool is executed.
    async fn pre_tool_call(&self, _info: &mut ToolCallInfo) -> PreToolAction {
        PreToolAction::Continue
    }

    /// Called after each tool completes.
    async fn post_tool_call(&self, _info: &mut ToolResultInfo) -> PostToolAction {
        PostToolAction::Continue
    }

    /// Called when a turn ends with no tool calls.
    async fn on_turn_end(&self, _history: &[Item]) -> TurnEndAction {
        TurnEndAction::Finish
    }

    /// Called when execution is interrupted (abort or cancel).
    async fn on_abort(&self, _reason: &str) {}
}

/// Default interceptor: no intervention. Worker proceeds through the loop
/// without any external control flow decisions.
pub(crate) struct DefaultInterceptor;

#[async_trait]
impl Interceptor for DefaultInterceptor {}
