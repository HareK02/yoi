//! CompactInterceptor — wraps HookInterceptor with urgent compaction check.
//!
//! Decorator that delegates all [`Interceptor`] methods to the inner
//! `HookInterceptor`, then adds a token-count check in `pre_llm_request`.
//! When `last_input_tokens` exceeds the turn threshold, returns
//! `PreRequestAction::Yield` so the Worker exits the turn loop cleanly
//! with `WorkerResult::Yielded` and Pod can perform compaction.

use std::sync::Arc;

use async_trait::async_trait;
use llm_worker::interceptor::{
    Interceptor, PostToolAction, PreRequestAction, PreToolAction, PromptAction, ToolCallInfo,
    ToolResultInfo, TurnEndAction,
};
use llm_worker::Item;
use tracing::info;

use crate::compact_state::CompactState;
use crate::hook_interceptor::HookInterceptor;

/// Interceptor that wraps HookInterceptor and adds between-turns
/// compaction threshold check.
pub(crate) struct CompactInterceptor {
    inner: HookInterceptor,
    state: Arc<CompactState>,
}

impl CompactInterceptor {
    pub(crate) fn new(inner: HookInterceptor, state: Arc<CompactState>) -> Self {
        Self { inner, state }
    }
}

#[async_trait]
impl Interceptor for CompactInterceptor {
    async fn on_prompt_submit(&self, item: &mut Item) -> PromptAction {
        self.inner.on_prompt_submit(item).await
    }

    async fn pre_llm_request(&self, context: &mut Vec<Item>) -> PreRequestAction {
        // Step 1: Delegate to inner (PruneHook and other hooks run first).
        let inner_action = self.inner.pre_llm_request(context).await;
        if !matches!(inner_action, PreRequestAction::Continue) {
            return inner_action;
        }

        // Step 2: Check between-turns compaction threshold.
        if !self.state.is_disabled() && self.state.exceeds_turn() {
            info!(
                input_tokens = self.state.last_input_tokens(),
                threshold = self.state.turn_threshold(),
                "Between-turns compaction threshold exceeded, yielding"
            );
            return PreRequestAction::Yield;
        }

        PreRequestAction::Continue
    }

    async fn pre_tool_call(&self, info: &mut ToolCallInfo) -> PreToolAction {
        self.inner.pre_tool_call(info).await
    }

    async fn post_tool_call(&self, info: &mut ToolResultInfo) -> PostToolAction {
        self.inner.post_tool_call(info).await
    }

    async fn on_turn_end(&self, history: &[Item]) -> TurnEndAction {
        self.inner.on_turn_end(history).await
    }

    async fn on_abort(&self, reason: &str) {
        self.inner.on_abort(reason).await;
    }
}
