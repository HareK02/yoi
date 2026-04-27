//! Pod-owned `Interceptor` implementation.
//!
//! Bridges Pod's internal mechanisms (compaction trigger today;
//! notification injection / output truncation in the future) and the
//! public `HookRegistry`. Internal mechanisms run first and have full
//! mutable access via the `Interceptor` trait. Hooks then receive
//! read-only summary information and only return control-flow
//! decisions (continue / skip / abort / pause).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use llm_worker::Item;
use llm_worker::interceptor::{
    Interceptor, PostToolAction, PreRequestAction, PreToolAction, PromptAction, ToolCallInfo,
    ToolResultInfo, TurnEndAction,
};
use llm_worker::tool::ToolOutput;
use session_store::UsageRecord;
use tracing::info;

use crate::compact::state::CompactState;
use crate::compact::token_counter::total_tokens_impl;
use crate::hook::{
    AbortInfo, HookRegistry, PreRequestInfo, PromptSubmitInfo, ToolCallSummary, ToolResultSummary,
    TurnEndInfo,
};
use crate::ipc::notify_buffer::{NotifyBuffer, format_notify};
use crate::prompt::catalog::PromptCatalog;
use tracing::warn;

/// Maximum number of bytes copied into `TurnEndInfo::final_text_preview`.
const FINAL_TEXT_PREVIEW_LIMIT: usize = 512;

pub(crate) struct PodInterceptor {
    registry: Arc<HookRegistry>,
    compact_state: Option<Arc<CompactState>>,
    /// Shared view of the cumulative UsageRecord timeline. Used with the
    /// per-request `context` to estimate current occupancy for threshold
    /// checks. `None` when compaction is disabled (both thresholds unset).
    usage_history: Option<Arc<Mutex<Vec<UsageRecord>>>>,
    /// Pending-notification buffer drained into the per-request
    /// context at the head of `pre_llm_request`.
    pending_notifies: NotifyBuffer,
    /// Prompt catalog used to render the injected notification wrapper.
    prompts: Arc<PromptCatalog>,
    /// Next turn index assigned by `on_prompt_submit`.
    next_turn_index: AtomicUsize,
    /// Tool calls observed in the current turn (reset on each new prompt).
    tool_calls_this_turn: AtomicUsize,
}

impl PodInterceptor {
    pub(crate) fn new(
        registry: Arc<HookRegistry>,
        compact_state: Option<Arc<CompactState>>,
        usage_history: Option<Arc<Mutex<Vec<UsageRecord>>>>,
        pending_notifies: NotifyBuffer,
        prompts: Arc<PromptCatalog>,
    ) -> Self {
        Self {
            registry,
            compact_state,
            usage_history,
            pending_notifies,
            prompts,
            next_turn_index: AtomicUsize::new(0),
            tool_calls_this_turn: AtomicUsize::new(0),
        }
    }

    fn current_turn_index(&self) -> usize {
        self.next_turn_index
            .load(Ordering::Relaxed)
            .saturating_sub(1)
    }

    /// Estimate current input-token occupancy for `context`, projected
    /// through the shared UsageRecord timeline. Returns `None` when
    /// `usage_history` is not attached (compaction fully disabled).
    fn estimated_tokens(&self, context: &[Item]) -> Option<u64> {
        let handle = self.usage_history.as_ref()?;
        let records = handle.lock().expect("usage_history poisoned").clone();
        Some(total_tokens_impl(context, &records).tokens)
    }
}

#[async_trait]
impl Interceptor for PodInterceptor {
    async fn on_prompt_submit(&self, item: &mut Item) -> PromptAction {
        let turn_index = self.next_turn_index.fetch_add(1, Ordering::Relaxed);
        self.tool_calls_this_turn.store(0, Ordering::Relaxed);

        let info = PromptSubmitInfo {
            input_text: extract_message_text(item).unwrap_or_default(),
            turn_index,
        };
        for hook in &self.registry.on_prompt_submit {
            let action = hook.call(&info).await;
            if !matches!(action, PromptAction::Continue) {
                return action;
            }
        }
        PromptAction::Continue
    }

    async fn pre_llm_request(&self, context: &mut Vec<Item>) -> PreRequestAction {
        let current_tokens = self.estimated_tokens(context);

        // Internal mechanism: between-requests compaction trigger (safety net).
        if let Some(state) = self.compact_state.as_ref() {
            if !state.is_disabled() {
                let current = current_tokens.unwrap_or(0);
                if state.exceeds_request(current) {
                    info!(
                        input_tokens = current,
                        threshold = state.request_threshold().unwrap_or(0),
                        "Between-requests compaction threshold exceeded, yielding"
                    );
                    return PreRequestAction::Yield;
                }
            }
        }

        // Internal mechanism: drain pending `Method::Notify` notifications
        // into the per-request context as transient system messages.
        // These are not persisted to the Worker history; they exist only
        // for this single LLM request.
        for n in self.pending_notifies.drain() {
            match format_notify(&n, &self.prompts) {
                Ok(item) => context.push(item),
                Err(e) => {
                    // A render failure here would starve the LLM of the
                    // notify text. Fall back to the raw message —
                    // it still carries the intent, just without the
                    // wrapper phrasing.
                    warn!(error = %e, "failed to render notify_wrapper; using raw message");
                    context.push(Item::system_message(n.message.clone()));
                }
            }
        }

        let info = PreRequestInfo {
            item_count: context.len(),
            estimated_tokens: current_tokens,
            turn_index: self.current_turn_index(),
            tool_calls_this_turn: self.tool_calls_this_turn.load(Ordering::Relaxed),
        };
        for hook in &self.registry.pre_llm_request {
            let action = hook.call(&info).await;
            if !matches!(action, PreRequestAction::Continue) {
                return action;
            }
        }
        PreRequestAction::Continue
    }

    async fn pre_tool_call(&self, info: &mut ToolCallInfo) -> PreToolAction {
        let summary = ToolCallSummary {
            call_id: info.call.id.clone(),
            tool_name: info.call.name.clone(),
            arguments: info.call.input.clone(),
        };
        for hook in &self.registry.pre_tool_call {
            let action = hook.call(&summary).await;
            if !matches!(action, PreToolAction::Continue) {
                return action;
            }
        }
        self.tool_calls_this_turn.fetch_add(1, Ordering::Relaxed);
        PreToolAction::Continue
    }

    async fn post_tool_call(&self, info: &mut ToolResultInfo) -> PostToolAction {
        let summary = ToolResultSummary {
            call_id: info.result.tool_use_id.clone(),
            tool_name: info.call.name.clone(),
            is_error: info.result.is_error,
            output: ToolOutput {
                summary: info.result.summary.clone(),
                content: info.result.content.clone(),
            },
        };
        for hook in &self.registry.post_tool_call {
            let action = hook.call(&summary).await;
            if !matches!(action, PostToolAction::Continue) {
                return action;
            }
        }
        PostToolAction::Continue
    }

    async fn on_turn_end(&self, history: &[Item]) -> TurnEndAction {
        let final_text_preview = history
            .iter()
            .rev()
            .find(|i| i.is_assistant_message())
            .and_then(extract_message_text)
            .map(|t| preview(&t, FINAL_TEXT_PREVIEW_LIMIT))
            .unwrap_or_default();
        let info = TurnEndInfo {
            turn_index: self.current_turn_index(),
            tool_calls_count: self.tool_calls_this_turn.load(Ordering::Relaxed),
            final_text_preview,
        };
        for hook in &self.registry.on_turn_end {
            let action = hook.call(&info).await;
            if !matches!(action, TurnEndAction::Finish) {
                return action;
            }
        }
        TurnEndAction::Finish
    }

    async fn on_abort(&self, reason: &str) {
        let info = AbortInfo {
            reason: reason.to_string(),
        };
        for hook in &self.registry.on_abort {
            hook.call(&info).await;
        }
    }
}

fn extract_message_text(item: &Item) -> Option<String> {
    match item {
        Item::Message { content, .. } => Some(
            content
                .iter()
                .map(|p| p.as_text())
                .collect::<Vec<_>>()
                .join(""),
        ),
        _ => None,
    }
}

fn preview(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    use super::*;
    use crate::hook::{Hook, HookRegistryBuilder, PreLlmRequest};

    struct CountingHook(Arc<AtomicUsize>);

    #[async_trait]
    impl Hook<PreLlmRequest> for CountingHook {
        async fn call(&self, _info: &PreRequestInfo) -> PreRequestAction {
            self.0.fetch_add(1, Ordering::Relaxed);
            PreRequestAction::Continue
        }
    }

    fn registry_with_pre_llm_hook(counter: Arc<AtomicUsize>) -> Arc<HookRegistry> {
        let mut builder = HookRegistryBuilder::new();
        builder.add_pre_llm_request(CountingHook(counter));
        Arc::new(builder.build())
    }

    /// Build a usage_history handle with a single record pinned at the
    /// current `context_len` so that `total_tokens_impl` returns exactly
    /// `tokens` (Measured, no interpolation or byte-based fallback).
    fn usage_handle_with(context_len: usize, tokens: u64) -> Arc<Mutex<Vec<UsageRecord>>> {
        Arc::new(Mutex::new(vec![UsageRecord {
            history_len: context_len,
            input_total_tokens: tokens,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            output_tokens: 0,
        }]))
    }

    #[tokio::test]
    async fn pre_llm_request_yields_and_skips_hooks_when_request_threshold_exceeded() {
        let count = Arc::new(AtomicUsize::new(0));
        let registry = registry_with_pre_llm_hook(count.clone());

        let state = Arc::new(CompactState::new(None, Some(100), 2));
        let ctx_items = vec![Item::user_message("hi")];
        let history = usage_handle_with(ctx_items.len(), 200);

        let interceptor = PodInterceptor::new(
            registry,
            Some(state),
            Some(history),
            NotifyBuffer::new(),
            PromptCatalog::builtins_only().unwrap(),
        );
        let mut ctx = ctx_items;
        let action = interceptor.pre_llm_request(&mut ctx).await;

        assert!(matches!(action, PreRequestAction::Yield));
        // Hook must not run when an internal mechanism short-circuits first.
        assert_eq!(count.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn pre_llm_request_runs_hooks_when_under_threshold() {
        let count = Arc::new(AtomicUsize::new(0));
        let registry = registry_with_pre_llm_hook(count.clone());

        let state = Arc::new(CompactState::new(None, Some(100), 2));
        let ctx_items = vec![Item::user_message("hi")];
        let history = usage_handle_with(ctx_items.len(), 50);

        let interceptor = PodInterceptor::new(
            registry,
            Some(state),
            Some(history),
            NotifyBuffer::new(),
            PromptCatalog::builtins_only().unwrap(),
        );
        let mut ctx = ctx_items;
        let action = interceptor.pre_llm_request(&mut ctx).await;

        assert!(matches!(action, PreRequestAction::Continue));
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn pre_llm_request_does_not_yield_when_only_post_run_threshold_set() {
        // request_threshold = None → safety-net check is inert inside the turn
        // even if current occupancy is huge. Post-run check runs elsewhere.
        let count = Arc::new(AtomicUsize::new(0));
        let registry = registry_with_pre_llm_hook(count.clone());

        let state = Arc::new(CompactState::new(Some(100), None, 2));
        let ctx_items = vec![Item::user_message("hi")];
        let history = usage_handle_with(ctx_items.len(), 10_000);

        let interceptor = PodInterceptor::new(
            registry,
            Some(state),
            Some(history),
            NotifyBuffer::new(),
            PromptCatalog::builtins_only().unwrap(),
        );
        let mut ctx = ctx_items;
        let action = interceptor.pre_llm_request(&mut ctx).await;

        assert!(matches!(action, PreRequestAction::Continue));
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn pre_llm_request_runs_hooks_when_no_compact_state() {
        let count = Arc::new(AtomicUsize::new(0));
        let registry = registry_with_pre_llm_hook(count.clone());

        let interceptor = PodInterceptor::new(
            registry,
            None,
            None,
            NotifyBuffer::new(),
            PromptCatalog::builtins_only().unwrap(),
        );
        let mut ctx: Vec<Item> = Vec::new();
        let action = interceptor.pre_llm_request(&mut ctx).await;

        assert!(matches!(action, PreRequestAction::Continue));
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }

    struct AbortingHook(Arc<AtomicBool>);

    #[async_trait]
    impl Hook<PreLlmRequest> for AbortingHook {
        async fn call(&self, _info: &PreRequestInfo) -> PreRequestAction {
            self.0.store(true, Ordering::Relaxed);
            PreRequestAction::Cancel("nope".into())
        }
    }

    #[tokio::test]
    async fn pre_llm_request_drains_pending_notifies_into_context() {
        let registry = Arc::new(HookRegistryBuilder::new().build());
        let buffer = NotifyBuffer::new();
        buffer.push("first".into());
        buffer.push("second".into());

        let interceptor = PodInterceptor::new(
            registry,
            None,
            None,
            buffer.clone(),
            PromptCatalog::builtins_only().unwrap(),
        );
        let mut ctx: Vec<Item> = vec![Item::user_message("hi")];
        let action = interceptor.pre_llm_request(&mut ctx).await;

        assert!(matches!(action, PreRequestAction::Continue));
        // Original user message preserved, two notifications appended in order.
        assert_eq!(ctx.len(), 3);
        let second = ctx[1].as_text().unwrap_or_default();
        let third = ctx[2].as_text().unwrap_or_default();
        assert!(second.contains("[Notification]"));
        assert!(second.contains("first"));
        assert!(third.contains("[Notification]"));
        assert!(third.contains("second"));
        // Buffer is drained after a single pre_llm_request call.
        assert!(buffer.is_empty());
    }

    #[tokio::test]
    async fn pre_llm_request_skips_notification_injection_when_yielding() {
        // When compaction yields, notifications remain in the buffer for
        // the next pre_llm_request (after compaction + resume).
        let registry = Arc::new(HookRegistryBuilder::new().build());
        let buffer = NotifyBuffer::new();
        buffer.push("msg".into());

        let state = Arc::new(CompactState::new(None, Some(100), 2));
        let ctx_items = vec![Item::user_message("hi")];
        let history = usage_handle_with(ctx_items.len(), 200);

        let interceptor = PodInterceptor::new(
            registry,
            Some(state),
            Some(history),
            buffer.clone(),
            PromptCatalog::builtins_only().unwrap(),
        );
        let mut ctx = ctx_items;
        let action = interceptor.pre_llm_request(&mut ctx).await;

        assert!(matches!(action, PreRequestAction::Yield));
        // Notifications were not drained (still held for post-compact resume).
        assert_eq!(ctx.len(), 1);
        assert_eq!(buffer.len(), 1);
    }

    #[tokio::test]
    async fn pre_llm_request_short_circuits_on_first_non_continue() {
        let first_called = Arc::new(AtomicBool::new(false));
        let second_count = Arc::new(AtomicUsize::new(0));
        let mut builder = HookRegistryBuilder::new();
        builder.add_pre_llm_request(AbortingHook(first_called.clone()));
        builder.add_pre_llm_request(CountingHook(second_count.clone()));
        let registry = Arc::new(builder.build());

        let interceptor = PodInterceptor::new(
            registry,
            None,
            None,
            NotifyBuffer::new(),
            PromptCatalog::builtins_only().unwrap(),
        );
        let mut ctx: Vec<Item> = Vec::new();
        let action = interceptor.pre_llm_request(&mut ctx).await;

        assert!(matches!(action, PreRequestAction::Cancel(_)));
        assert!(first_called.load(Ordering::Relaxed));
        assert_eq!(second_count.load(Ordering::Relaxed), 0);
    }
}
