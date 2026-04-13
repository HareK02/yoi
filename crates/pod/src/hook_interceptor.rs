//! HookInterceptor — bridges Pod-layer hooks to Worker's Interceptor trait.

use std::sync::Arc;

use async_trait::async_trait;
use llm_worker::Item;
use llm_worker::interceptor::{
    Interceptor, PostToolAction, PreRequestAction, PreToolAction, PromptAction, ToolCallInfo,
    ToolResultInfo, TurnEndAction,
};

use crate::hook::HookRegistry;

/// An `Interceptor` implementation that delegates to a `HookRegistry`.
///
/// Each method iterates the registered hooks in order and short-circuits
/// on the first non-Continue (or non-Finish) result.
pub(crate) struct HookInterceptor {
    registry: Arc<HookRegistry>,
}

impl HookInterceptor {
    pub(crate) fn new(registry: Arc<HookRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Interceptor for HookInterceptor {
    async fn on_prompt_submit(&self, item: &mut Item) -> PromptAction {
        for hook in &self.registry.on_prompt_submit {
            let action = hook.call(item).await;
            if !matches!(action, PromptAction::Continue) {
                return action;
            }
        }
        PromptAction::Continue
    }

    async fn pre_llm_request(&self, context: &mut Vec<Item>) -> PreRequestAction {
        for hook in &self.registry.pre_llm_request {
            let action = hook.call(context).await;
            if !matches!(action, PreRequestAction::Continue) {
                return action;
            }
        }
        PreRequestAction::Continue
    }

    async fn pre_tool_call(&self, info: &mut ToolCallInfo) -> PreToolAction {
        for hook in &self.registry.pre_tool_call {
            let action = hook.call(info).await;
            if !matches!(action, PreToolAction::Continue) {
                return action;
            }
        }
        PreToolAction::Continue
    }

    async fn post_tool_call(&self, info: &mut ToolResultInfo) -> PostToolAction {
        for hook in &self.registry.post_tool_call {
            let action = hook.call(info).await;
            if !matches!(action, PostToolAction::Continue) {
                return action;
            }
        }
        PostToolAction::Continue
    }

    async fn on_turn_end(&self, history: &[Item]) -> TurnEndAction {
        let mut history_vec = history.to_vec();
        for hook in &self.registry.on_turn_end {
            let action = hook.call(&mut history_vec).await;
            if !matches!(action, TurnEndAction::Finish) {
                return action;
            }
        }
        TurnEndAction::Finish
    }

    async fn on_abort(&self, reason: &str) {
        let mut reason_string = reason.to_string();
        for hook in &self.registry.on_abort {
            hook.call(&mut reason_string).await;
        }
    }
}
