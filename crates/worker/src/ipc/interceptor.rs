//! Worker-owned `Interceptor` implementation.
//!
//! Bridges Worker's internal mechanisms (compaction trigger today;
//! notification injection / output truncation in the future) and the
//! public `HookRegistry`. Internal mechanisms run first and have full
//! mutable access via the `Interceptor` trait. Hooks then receive
//! event-specific read-only contexts and only return control-flow
//! decisions (continue / skip / abort / pause).

use std::borrow::Cow;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use llm_engine::Item;
use llm_engine::UsageRecord;
use llm_engine::interceptor::{
    Interceptor, PostToolAction, PreRequestAction, PreToolAction, PromptAction, ToolCallInfo,
    ToolResultInfo, TurnEndAction,
};
use llm_engine::tool::ToolOutput;
use tracing::info;
use tracing::warn;

use crate::compact::state::CompactState;
use crate::compact::usage_tracker::UsageTracker;
use session_store::SystemItem;

use crate::hook::{
    AbortInfo, HookPostToolAction, HookPreRequestAction, HookPreToolAction, HookPromptAction,
    HookRegistry, HookTurnEndAction, PreRequestContext, PreRequestInfo, PromptSubmitInfo,
    SystemItemAppendHandle, ToolCallSummary, ToolResultSummary, TurnEndInfo,
};
use crate::ipc::notify_buffer::{NotifyBuffer, build_system_item};
use crate::prompt::catalog::PromptCatalog;
use crate::worker::SystemItemCommitter;
use llm_engine::token_counter::total_tokens;

/// Maximum number of bytes copied into `TurnEndInfo::final_text_preview`.
const FINAL_TEXT_PREVIEW_LIMIT: usize = 512;

pub(crate) struct WorkerInterceptor {
    registry: Arc<HookRegistry>,
    compact_state: Option<Arc<CompactState>>,
    /// Shared view of the cumulative UsageRecord timeline. Used with the
    /// per-request `context` to estimate current occupancy for threshold
    /// checks. `None` when compaction is disabled (both thresholds unset).
    usage_history: Option<Arc<Mutex<Vec<UsageRecord>>>>,
    /// In-flight usage records observed during the current run but not yet
    /// persisted into `usage_history`. Subsequent tool-loop LLM calls must
    /// see these records during pre-request safety accounting.
    usage_tracker: Option<Arc<UsageTracker>>,
    /// Pending-notification buffer drained into `worker.history`
    /// via [`Self::pending_history_appends`] just before the next LLM
    /// request. The Engine `extend`s these into its persistent history
    /// so the LLM has a visible trigger for any reaction it commits.
    pending_notifies: NotifyBuffer,
    /// Submit-scoped stash of resolver-produced typed system items.
    /// Drained inside `on_prompt_submit`, committed as
    /// `LogEntry::SystemItem` entries through `log_writer`, and
    /// returned to the worker as `Item::system_message` via
    /// `PromptAction::ContinueWith`. Populated by `Worker::run`
    /// immediately before handing off to the worker.
    pending_attachments: Arc<Mutex<Vec<SystemItem>>>,
    /// Prompt catalog used to render pending notification entries into the
    /// same system-message text that will be persisted in history.
    prompts: Arc<PromptCatalog>,
    /// Type-erased commit handle. The interceptor uses it to commit
    /// `LogEntry::SystemItem` entries directly (sync) before
    /// returning the corresponding `Item::system_message`s up to the
    /// worker. `None` in tests / `Worker::new` paths where no writer is
    /// attached.
    log_writer: Option<Arc<dyn SystemItemCommitter>>,
    /// Next turn index assigned by `on_prompt_submit`.
    next_turn_index: AtomicUsize,
    /// Tool calls observed in the current turn (reset on each new prompt).
    tool_calls_this_turn: AtomicUsize,
}

impl WorkerInterceptor {
    pub(crate) fn new(
        registry: Arc<HookRegistry>,
        compact_state: Option<Arc<CompactState>>,
        usage_history: Option<Arc<Mutex<Vec<UsageRecord>>>>,
        pending_notifies: NotifyBuffer,
        pending_attachments: Arc<Mutex<Vec<SystemItem>>>,
        prompts: Arc<PromptCatalog>,
        log_writer: Option<Arc<dyn SystemItemCommitter>>,
    ) -> Self {
        Self {
            registry,
            compact_state,
            usage_history,
            usage_tracker: None,
            pending_notifies,
            pending_attachments,
            prompts,
            log_writer,
            next_turn_index: AtomicUsize::new(0),
            tool_calls_this_turn: AtomicUsize::new(0),
        }
    }

    pub(crate) fn with_usage_tracker(mut self, usage_tracker: Arc<UsageTracker>) -> Self {
        self.usage_tracker = Some(usage_tracker);
        self
    }

    /// Commit each `SystemItem` as its own `LogEntry::SystemItem`
    /// entry through the attached writer (no-op when no writer is
    /// wired). Sync — writes complete before the matching
    /// `Item::system_message`s reach the worker via
    /// `ContinueWith` / `pending_history_appends`, so on-disk order
    /// matches worker-history order.
    fn commit_system_items(&self, items: &[SystemItem]) -> Result<(), session_store::StoreError> {
        let Some(writer) = self.log_writer.as_ref() else {
            return Ok(());
        };
        for item in items {
            writer.commit_system_item(item.clone())?;
        }
        Ok(())
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
        let mut records = handle.lock().expect("usage_history poisoned").clone();
        if let Some(tracker) = self.usage_tracker.as_ref() {
            records.extend(tracker.records());
        }
        Some(total_tokens(context, &records).tokens)
    }

    fn request_threshold_exceeded(&self, current_tokens: Option<u64>, context: &[Item]) -> bool {
        if let Some(state) = self.compact_state.as_ref() {
            if !state.is_disabled() && !state.just_compacted() {
                let current = current_tokens.unwrap_or(0);
                if state.exceeds_request(current) {
                    let shape = context_shape(context);
                    info!(
                        input_tokens = current,
                        threshold = state.request_threshold().unwrap_or(0),
                        items_len = shape.items_len,
                        items_json_bytes = shape.items_json_bytes,
                        reasoning_items = shape.reasoning_items,
                        reasoning_encrypted_content_count = shape.reasoning_encrypted_content_count,
                        reasoning_encrypted_content_bytes = shape.reasoning_encrypted_content_bytes,
                        "Between-requests compaction threshold exceeded, yielding"
                    );
                    return true;
                }
            }
        }
        false
    }
}

#[async_trait]
impl Interceptor for WorkerInterceptor {
    async fn on_prompt_submit(&self, item: &mut Item) -> PromptAction {
        let turn_index = self.next_turn_index.fetch_add(1, Ordering::Relaxed);
        self.tool_calls_this_turn.store(0, Ordering::Relaxed);

        let info = PromptSubmitInfo {
            input_text: extract_message_text(item).unwrap_or_default(),
            turn_index,
        };
        for hook in &self.registry.on_prompt_submit {
            let action = hook.call(&info).await;
            if !matches!(action, HookPromptAction::Continue) {
                return action.into();
            }
        }
        let extras: Vec<SystemItem> = std::mem::take(
            &mut *self
                .pending_attachments
                .lock()
                .expect("pending_attachments poisoned"),
        );
        if extras.is_empty() {
            PromptAction::Continue
        } else {
            // Commit the typed system items first, then hand the
            // matching `Item::system_message`s to the worker. Sync
            // commits land BEFORE the worker pushes its
            // `Item::system_message`s, so on-disk order matches
            // worker-history order.
            let items: Vec<Item> = extras.iter().map(SystemItem::to_history_item).collect();
            match self.commit_system_items(&extras) {
                Ok(()) => PromptAction::ContinueWith(items),
                Err(error) => PromptAction::Cancel(format!("session persistence failed: {error}")),
            }
        }
    }

    async fn pending_history_appends(&self) -> Result<Vec<Item>, String> {
        let drained = self.pending_notifies.drain();
        if drained.is_empty() {
            return Ok(Vec::new());
        }

        let mut system_items: Vec<SystemItem> = Vec::with_capacity(drained.len());
        let mut items: Vec<Item> = Vec::with_capacity(drained.len());
        for entry in drained {
            match build_system_item(&entry, &self.prompts) {
                Ok(system_item) => {
                    items.push(system_item.to_history_item());
                    system_items.push(system_item);
                }
                Err(e) => {
                    // A render failure here would starve the LLM of
                    // the notify text. Fall back to a raw item so the
                    // trigger still lands in history; the entry will
                    // simply be skipped from the SystemItem batch.
                    warn!(error = %e, "failed to render notify_wrapper; using raw message");
                    let fallback = match &entry {
                        super::notify_buffer::PendingNotify::Notify { message, .. } => {
                            message.clone()
                        }
                        super::notify_buffer::PendingNotify::WorkerEvent { event } => {
                            session_store::render_worker_event(event)
                        }
                    };
                    items.push(Item::system_message(fallback));
                }
            }
        }
        self.commit_system_items(&system_items)
            .map_err(|error| format!("session persistence failed: {error}"))?;
        Ok(items)
    }

    async fn pre_llm_request(&self, context: &mut Vec<Item>) -> PreRequestAction {
        let initial_tokens = self.estimated_tokens(context);
        if self.request_threshold_exceeded(initial_tokens, context) {
            return PreRequestAction::Yield;
        }
        let info = PreRequestInfo {
            item_count: context.len(),
            estimated_tokens: initial_tokens,
            turn_index: self.current_turn_index(),
            tool_calls_this_turn: self.tool_calls_this_turn.load(Ordering::Relaxed),
        };
        let pending_hook_system_items = Arc::new(Mutex::new(Vec::new()));
        let system_item_sink = self
            .log_writer
            .as_ref()
            .map(|_| SystemItemAppendHandle::new(Arc::clone(&pending_hook_system_items)));
        let hook_context = PreRequestContext::new(info, system_item_sink);
        for hook in &self.registry.pre_llm_request {
            let action = hook.call(&hook_context).await;
            if !matches!(action, HookPreRequestAction::Continue) {
                return action.into();
            }
        }

        let system_items: Vec<SystemItem> = std::mem::take(
            &mut *pending_hook_system_items
                .lock()
                .expect("pending hook system-item queue poisoned"),
        );
        let appended_items: Vec<Item> = system_items
            .iter()
            .map(SystemItem::to_history_item)
            .collect();
        let effective_context = if appended_items.is_empty() {
            Cow::Borrowed(context.as_slice())
        } else {
            let mut effective = context.clone();
            effective.extend(appended_items.clone());
            Cow::Owned(effective)
        };
        let current_tokens = self.estimated_tokens(effective_context.as_ref());

        if self.request_threshold_exceeded(current_tokens, effective_context.as_ref()) {
            if let Err(error) = self.commit_system_items(&system_items) {
                return PreRequestAction::Cancel(format!("session persistence failed: {error}"));
            }
            return if appended_items.is_empty() {
                PreRequestAction::Yield
            } else {
                PreRequestAction::YieldWith(appended_items)
            };
        }

        if let Some(usage_tracker) = self.usage_tracker.as_ref() {
            usage_tracker.note_request(effective_context.len());
        }
        if system_items.is_empty() {
            return PreRequestAction::Continue;
        }
        match self.commit_system_items(&system_items) {
            Ok(()) => PreRequestAction::ContinueWith(appended_items),
            Err(error) => PreRequestAction::Cancel(format!("session persistence failed: {error}")),
        }
    }

    async fn pre_tool_call(&self, info: &mut ToolCallInfo) -> PreToolAction {
        let summary = ToolCallSummary {
            call_id: info.call.id.clone(),
            tool_name: info.call.name.clone(),
            arguments: info.call.input.clone(),
        };
        for hook in &self.registry.pre_tool_call {
            let action = hook.call(&summary).await;
            if !matches!(action, HookPreToolAction::Continue) {
                return action.into_worker_action(summary.call_id.clone());
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
            if !matches!(action, HookPostToolAction::Continue) {
                return action.into();
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
            if !matches!(action, HookTurnEndAction::Finish) {
                return action.into();
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

struct ContextShape {
    items_len: usize,
    items_json_bytes: Option<usize>,
    reasoning_items: usize,
    reasoning_encrypted_content_count: usize,
    reasoning_encrypted_content_bytes: usize,
}

fn context_shape(context: &[Item]) -> ContextShape {
    let mut shape = ContextShape {
        items_len: context.len(),
        items_json_bytes: serde_json::to_vec(context).ok().map(|bytes| bytes.len()),
        reasoning_items: 0,
        reasoning_encrypted_content_count: 0,
        reasoning_encrypted_content_bytes: 0,
    };
    for item in context {
        if let Item::Reasoning {
            encrypted_content, ..
        } = item
        {
            shape.reasoning_items += 1;
            if let Some(encrypted) = encrypted_content {
                shape.reasoning_encrypted_content_count += 1;
                shape.reasoning_encrypted_content_bytes += encrypted.len();
            }
        }
    }
    shape
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
    use crate::feature::FeatureRegistryBuilder;
    use crate::feature::builtin::TaskFeature;
    use crate::hook::{
        Hook, HookPostToolAction, HookPreRequestAction, HookPreToolAction, HookRegistryBuilder,
        HookTurnEndAction, OnTurnEnd, PostToolCall, PreLlmRequest, PreToolCall,
    };

    struct CountingHook(Arc<AtomicUsize>);

    #[async_trait]
    impl Hook<PreLlmRequest> for CountingHook {
        async fn call(&self, _info: &PreRequestContext) -> HookPreRequestAction {
            self.0.fetch_add(1, Ordering::Relaxed);
            HookPreRequestAction::Continue
        }
    }

    fn registry_with_pre_llm_hook(count: Arc<AtomicUsize>) -> Arc<HookRegistry> {
        let mut builder = HookRegistryBuilder::new();
        builder.add_pre_llm_request(CountingHook(count));
        Arc::new(builder.build())
    }

    struct RecordingSystemItemCommitter {
        committed: Arc<Mutex<Vec<SystemItem>>>,
    }

    impl SystemItemCommitter for RecordingSystemItemCommitter {
        fn commit_log_entry(
            &self,
            entry: session_store::LogEntry,
        ) -> Result<(), session_store::StoreError> {
            if let session_store::LogEntry::SystemItem { item, .. } = entry {
                self.committed
                    .lock()
                    .expect("committed system-item list poisoned")
                    .push(item);
            }
            Ok(())
        }
    }

    struct AppendingPreRequestHook {
        saw_handle: Arc<AtomicBool>,
    }

    #[async_trait]
    impl Hook<PreLlmRequest> for AppendingPreRequestHook {
        async fn call(&self, input: &PreRequestContext) -> HookPreRequestAction {
            if let Some(system_items) = input.system_items() {
                self.saw_handle.store(true, Ordering::Relaxed);
                system_items.append_task_reminder("hook reminder");
            }
            HookPreRequestAction::Continue
        }
    }

    fn task_tool_call_info(name: &str, input: serde_json::Value) -> ToolCallInfo {
        let def = crate::feature::builtin::task::task_tools(
            crate::feature::builtin::task::TaskStore::new(),
        )
        .into_iter()
        .find(|def| {
            let (meta, _) = def();
            meta.name == name
        })
        .expect("task tool definition");
        let (meta, tool) = def();
        ToolCallInfo {
            call: llm_engine::tool::ToolCall {
                id: "call-id".into(),
                name: name.into(),
                input,
            },
            meta,
            tool,
            context: llm_engine::tool::ToolExecutionContext::new("call-id", "test-batch", 0),
        }
    }

    /// Build a usage_history handle with a single record pinned at the
    /// current `context_len` so that `total_tokens` returns exactly
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

        let interceptor = WorkerInterceptor::new(
            registry,
            Some(state),
            Some(history),
            NotifyBuffer::new(),
            Arc::new(Mutex::new(Vec::new())),
            PromptCatalog::builtins_only().unwrap(),
            None,
        );
        let mut ctx = ctx_items;
        let action = interceptor.pre_llm_request(&mut ctx).await;

        assert!(matches!(action, PreRequestAction::Yield));
        // Hook must not run when an internal mechanism short-circuits first.
        assert_eq!(count.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn pre_llm_request_yields_with_hook_appends_when_post_append_threshold_exceeded() {
        let saw_handle = Arc::new(AtomicBool::new(false));
        let mut builder = HookRegistryBuilder::new();
        builder.add_pre_llm_request(AppendingPreRequestHook {
            saw_handle: Arc::clone(&saw_handle),
        });
        let registry = Arc::new(builder.build());
        let state = Arc::new(CompactState::new(None, Some(50), 2));
        let ctx_items = vec![Item::user_message("hi")];
        let history = usage_handle_with(ctx_items.len(), 50);
        let committed = Arc::new(Mutex::new(Vec::new()));

        let interceptor = WorkerInterceptor::new(
            registry,
            Some(state),
            Some(history),
            NotifyBuffer::new(),
            Arc::new(Mutex::new(Vec::new())),
            PromptCatalog::builtins_only().unwrap(),
            Some(Arc::new(RecordingSystemItemCommitter {
                committed: Arc::clone(&committed),
            })),
        );
        let mut ctx = ctx_items;
        let action = interceptor.pre_llm_request(&mut ctx).await;

        match action {
            PreRequestAction::YieldWith(items) => assert_eq!(items.len(), 1),
            other => panic!("expected YieldWith queued system item, got {other:?}"),
        }
        assert!(saw_handle.load(Ordering::Relaxed));
        assert_eq!(committed.lock().expect("committed system items").len(), 1);
    }

    #[tokio::test]
    async fn pre_llm_request_counts_in_flight_usage_records() {
        let registry = Arc::new(HookRegistryBuilder::new().build());
        let state = Arc::new(CompactState::new(None, Some(100), 2));
        let ctx_items = vec![Item::user_message("hi")];
        let history = usage_handle_with(ctx_items.len(), 50);
        let usage_tracker = Arc::new(UsageTracker::new());
        usage_tracker.note_request(ctx_items.len());
        usage_tracker.record_usage(&llm_engine::event::UsageEvent {
            input_tokens: Some(150),
            output_tokens: Some(0),
            total_tokens: Some(150),
            cache_read_input_tokens: Some(0),
            cache_creation_input_tokens: Some(0),
        });

        let interceptor = WorkerInterceptor::new(
            registry,
            Some(state),
            Some(history),
            NotifyBuffer::new(),
            Arc::new(Mutex::new(Vec::new())),
            PromptCatalog::builtins_only().unwrap(),
            None,
        )
        .with_usage_tracker(usage_tracker);
        let mut ctx = ctx_items;
        let action = interceptor.pre_llm_request(&mut ctx).await;

        assert!(matches!(action, PreRequestAction::Yield));
    }

    #[tokio::test]
    async fn pre_llm_request_runs_hooks_when_under_threshold() {
        let count = Arc::new(AtomicUsize::new(0));
        let registry = registry_with_pre_llm_hook(count.clone());

        let state = Arc::new(CompactState::new(None, Some(100), 2));
        let ctx_items = vec![Item::user_message("hi")];
        let history = usage_handle_with(ctx_items.len(), 50);

        let interceptor = WorkerInterceptor::new(
            registry,
            Some(state),
            Some(history),
            NotifyBuffer::new(),
            Arc::new(Mutex::new(Vec::new())),
            PromptCatalog::builtins_only().unwrap(),
            None,
        );
        let mut ctx = ctx_items;
        let action = interceptor.pre_llm_request(&mut ctx).await;

        assert!(matches!(action, PreRequestAction::Continue));
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn pre_llm_request_does_not_yield_from_single_measurement_history_rate_projection() {
        let count = Arc::new(AtomicUsize::new(0));
        let registry = registry_with_pre_llm_hook(count.clone());
        let ctx_items = vec![
            Item::user_message("first"),
            Item::user_message("tool output ".repeat(400)),
        ];
        let record = UsageRecord {
            history_len: 1,
            input_total_tokens: 11_124,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            output_tokens: 0,
        };
        let prefix = llm_engine::token_counter::prefix_bytes(&ctx_items);
        let delta_bytes = prefix[2].saturating_sub(prefix[1]);
        let old_projection =
            11_124 + (delta_bytes as u128 * 11_124_u128 / prefix[1] as u128) as u64;
        let corrected = total_tokens(&ctx_items, std::slice::from_ref(&record)).tokens;
        let threshold = corrected + 100;
        assert!(old_projection > threshold);

        let state = Arc::new(CompactState::new(None, Some(threshold), 2));
        let history = Arc::new(Mutex::new(vec![record]));
        let interceptor = WorkerInterceptor::new(
            registry,
            Some(state),
            Some(history),
            NotifyBuffer::new(),
            Arc::new(Mutex::new(Vec::new())),
            PromptCatalog::builtins_only().unwrap(),
            None,
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

        let interceptor = WorkerInterceptor::new(
            registry,
            Some(state),
            Some(history),
            NotifyBuffer::new(),
            Arc::new(Mutex::new(Vec::new())),
            PromptCatalog::builtins_only().unwrap(),
            None,
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

        let interceptor = WorkerInterceptor::new(
            registry,
            None,
            None,
            NotifyBuffer::new(),
            Arc::new(Mutex::new(Vec::new())),
            PromptCatalog::builtins_only().unwrap(),
            None,
        );
        let mut ctx: Vec<Item> = Vec::new();
        let action = interceptor.pre_llm_request(&mut ctx).await;

        assert!(matches!(action, PreRequestAction::Continue));
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn pre_llm_request_commits_hook_system_items_before_continue_with() {
        let saw_handle = Arc::new(AtomicBool::new(false));
        let mut builder = HookRegistryBuilder::new();
        builder.add_pre_llm_request(AppendingPreRequestHook {
            saw_handle: Arc::clone(&saw_handle),
        });
        let registry = Arc::new(builder.build());
        let committed = Arc::new(Mutex::new(Vec::new()));
        let committer = Arc::new(RecordingSystemItemCommitter {
            committed: Arc::clone(&committed),
        });
        let interceptor = WorkerInterceptor::new(
            registry,
            None,
            None,
            NotifyBuffer::new(),
            Arc::new(Mutex::new(Vec::new())),
            PromptCatalog::builtins_only().unwrap(),
            Some(committer),
        );

        let mut ctx: Vec<Item> = Vec::new();
        let action = interceptor.pre_llm_request(&mut ctx).await;

        assert!(saw_handle.load(Ordering::Relaxed));
        let PreRequestAction::ContinueWith(items) = action else {
            panic!("expected ContinueWith for committed hook system item");
        };
        assert_eq!(items.len(), 1);
        assert!(matches!(
            &items[0],
            Item::Message {
                role: llm_engine::Role::System,
                ..
            }
        ));
        assert!(
            extract_message_text(&items[0])
                .expect("system message text")
                .contains("hook reminder")
        );
        let committed = committed
            .lock()
            .expect("committed system-item list poisoned");
        assert_eq!(committed.len(), 1);
        match &committed[0] {
            SystemItem::TaskReminder { body, .. } => assert!(body.contains("hook reminder")),
            other => panic!("unexpected committed system item: {other:?}"),
        }
    }

    #[tokio::test]
    async fn pre_llm_request_without_log_writer_does_not_expose_system_item_handle() {
        let saw_handle = Arc::new(AtomicBool::new(false));
        let mut builder = HookRegistryBuilder::new();
        builder.add_pre_llm_request(AppendingPreRequestHook {
            saw_handle: Arc::clone(&saw_handle),
        });
        let interceptor = WorkerInterceptor::new(
            Arc::new(builder.build()),
            None,
            None,
            NotifyBuffer::new(),
            Arc::new(Mutex::new(Vec::new())),
            PromptCatalog::builtins_only().unwrap(),
            None,
        );

        let mut ctx: Vec<Item> = Vec::new();
        let action = interceptor.pre_llm_request(&mut ctx).await;

        assert!(!saw_handle.load(Ordering::Relaxed));
        assert!(matches!(action, PreRequestAction::Continue));
    }

    struct AbortingHook(Arc<AtomicBool>);

    #[async_trait]
    impl Hook<PreLlmRequest> for AbortingHook {
        async fn call(&self, _info: &PreRequestContext) -> HookPreRequestAction {
            self.0.store(true, Ordering::Relaxed);
            HookPreRequestAction::Cancel("nope".into())
        }
    }

    #[tokio::test]
    async fn public_pre_tool_hook_deny_becomes_synthetic_error_and_short_circuits() {
        struct DenyToolHook(Arc<AtomicUsize>);
        struct CountingToolHook(Arc<AtomicUsize>);

        #[async_trait]
        impl Hook<PreToolCall> for DenyToolHook {
            async fn call(&self, input: &ToolCallSummary) -> HookPreToolAction {
                self.0.fetch_add(1, Ordering::Relaxed);
                assert_eq!(input.call_id, "call-id");
                assert_eq!(input.tool_name, "TaskList");
                assert_eq!(input.arguments, serde_json::json!({"scope": "all"}));
                HookPreToolAction::Deny("blocked by public hook".into())
            }
        }

        #[async_trait]
        impl Hook<PreToolCall> for CountingToolHook {
            async fn call(&self, _input: &ToolCallSummary) -> HookPreToolAction {
                self.0.fetch_add(1, Ordering::Relaxed);
                HookPreToolAction::Continue
            }
        }

        let first_count = Arc::new(AtomicUsize::new(0));
        let second_count = Arc::new(AtomicUsize::new(0));
        let mut builder = HookRegistryBuilder::new();
        builder.add_pre_tool_call(DenyToolHook(first_count.clone()));
        builder.add_pre_tool_call(CountingToolHook(second_count.clone()));
        let registry = Arc::new(builder.build());
        let interceptor = WorkerInterceptor::new(
            registry,
            None,
            None,
            NotifyBuffer::new(),
            Arc::new(Mutex::new(Vec::new())),
            PromptCatalog::builtins_only().unwrap(),
            None,
        );
        let mut info = task_tool_call_info("TaskList", serde_json::json!({"scope": "all"}));

        let action = interceptor.pre_tool_call(&mut info).await;

        match action {
            PreToolAction::SyntheticResult(result) => {
                assert_eq!(result.tool_use_id, "call-id");
                assert_eq!(result.summary, "blocked by public hook");
                assert_eq!(result.content, None);
                assert!(result.is_error);
            }
            other => panic!("expected synthetic denial, got {other:?}"),
        }
        assert_eq!(first_count.load(Ordering::Relaxed), 1);
        assert_eq!(second_count.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn public_post_tool_hooks_observe_output_but_only_abort() {
        struct AbortAfterToolHook(Arc<AtomicUsize>);

        #[async_trait]
        impl Hook<PostToolCall> for AbortAfterToolHook {
            async fn call(&self, input: &ToolResultSummary) -> HookPostToolAction {
                self.0.fetch_add(1, Ordering::Relaxed);
                assert_eq!(input.call_id, "call-id");
                assert_eq!(input.tool_name, "TaskList");
                assert!(!input.is_error);
                assert_eq!(input.output.summary, "ok");
                assert_eq!(input.output.content.as_deref(), Some("full"));
                HookPostToolAction::Abort("post tool abort".into())
            }
        }

        let count = Arc::new(AtomicUsize::new(0));
        let mut builder = HookRegistryBuilder::new();
        builder.add_post_tool_call(AbortAfterToolHook(count.clone()));
        let registry = Arc::new(builder.build());
        let interceptor = WorkerInterceptor::new(
            registry,
            None,
            None,
            NotifyBuffer::new(),
            Arc::new(Mutex::new(Vec::new())),
            PromptCatalog::builtins_only().unwrap(),
            None,
        );
        let info = task_tool_call_info("TaskList", serde_json::json!({}));
        let mut result_info = ToolResultInfo {
            call: info.call,
            result: llm_engine::tool::ToolResult::from_output(
                "call-id",
                ToolOutput {
                    summary: "ok".into(),
                    content: Some("full".into()),
                },
            ),
            meta: info.meta,
            tool: info.tool,
            context: info.context,
        };

        let action = interceptor.post_tool_call(&mut result_info).await;

        assert_eq!(action, PostToolAction::Abort("post tool abort".to_string()));
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn public_turn_end_hooks_are_observational_or_pause_only() {
        struct PauseTurnEndHook(Arc<AtomicUsize>);

        #[async_trait]
        impl Hook<OnTurnEnd> for PauseTurnEndHook {
            async fn call(&self, input: &TurnEndInfo) -> HookTurnEndAction {
                self.0.fetch_add(1, Ordering::Relaxed);
                assert_eq!(input.turn_index, 0);
                assert_eq!(input.tool_calls_count, 0);
                assert_eq!(input.final_text_preview, "done");
                HookTurnEndAction::Pause
            }
        }

        let count = Arc::new(AtomicUsize::new(0));
        let mut builder = HookRegistryBuilder::new();
        builder.add_on_turn_end(PauseTurnEndHook(count.clone()));
        let registry = Arc::new(builder.build());
        let interceptor = WorkerInterceptor::new(
            registry,
            None,
            None,
            NotifyBuffer::new(),
            Arc::new(Mutex::new(Vec::new())),
            PromptCatalog::builtins_only().unwrap(),
            None,
        );
        let history = vec![Item::user_message("hi"), Item::assistant_message("done")];

        let action = interceptor.on_turn_end(&history).await;

        assert!(matches!(action, TurnEndAction::Pause));
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn task_reminder_hook_append_is_counted_in_usage_request_len() {
        let feature = TaskFeature::from_history(&[Item::tool_call(
            "task-create-call",
            "TaskCreate",
            r#"{"subject":"track active work","description":"exercise reminder path"}"#,
        )]);
        let mut hook_builder = HookRegistryBuilder::new();
        let mut pending_tools = Vec::new();
        FeatureRegistryBuilder::new()
            .with_module(feature)
            .install_into_pending(&mut pending_tools, &mut hook_builder);
        let registry = Arc::new(hook_builder.build());
        let usage_tracker = Arc::new(UsageTracker::new());
        let committed = Arc::new(Mutex::new(Vec::new()));
        let interceptor = WorkerInterceptor::new(
            registry,
            None,
            None,
            NotifyBuffer::new(),
            Arc::new(Mutex::new(Vec::new())),
            PromptCatalog::builtins_only().unwrap(),
            Some(Arc::new(RecordingSystemItemCommitter {
                committed: Arc::clone(&committed),
            })),
        )
        .with_usage_tracker(Arc::clone(&usage_tracker));

        let ctx_items = vec![Item::user_message("hi")];
        for _ in 0..23 {
            let mut ctx = ctx_items.clone();
            let action = interceptor.pre_llm_request(&mut ctx).await;
            assert!(matches!(action, PreRequestAction::Continue));
            usage_tracker.record_usage(&llm_engine::event::UsageEvent {
                input_tokens: Some(10),
                output_tokens: Some(0),
                total_tokens: Some(10),
                cache_read_input_tokens: Some(0),
                cache_creation_input_tokens: Some(0),
            });
        }

        let mut ctx = ctx_items.clone();
        let action = interceptor.pre_llm_request(&mut ctx).await;
        let appended_len = match action {
            PreRequestAction::ContinueWith(items) => items.len(),
            other => panic!("expected reminder append, got {other:?}"),
        };
        assert_eq!(appended_len, 1);
        usage_tracker.record_usage(&llm_engine::event::UsageEvent {
            input_tokens: Some(11),
            output_tokens: Some(0),
            total_tokens: Some(11),
            cache_read_input_tokens: Some(0),
            cache_creation_input_tokens: Some(0),
        });

        let records = usage_tracker.records();
        assert_eq!(records.last().expect("usage record").history_len, 2);
        let committed = committed
            .lock()
            .expect("committed system-item list poisoned");
        assert_eq!(committed.len(), 1);
        let SystemItem::TaskReminder { body, .. } = &committed[0] else {
            panic!("expected task reminder, got {:?}", committed[0]);
        };
        assert!(body.contains("track active work"));
    }

    #[tokio::test]
    async fn pending_history_appends_drains_buffer_into_items() {
        let registry = Arc::new(HookRegistryBuilder::new().build());
        let buffer = NotifyBuffer::new();
        buffer.push_notify("first".into(), false);
        buffer.push_notify("second".into(), false);

        let interceptor = WorkerInterceptor::new(
            registry,
            None,
            None,
            buffer.clone(),
            Arc::new(Mutex::new(Vec::new())),
            PromptCatalog::builtins_only().unwrap(),
            None,
        );

        let items = interceptor.pending_history_appends().await.unwrap();
        assert_eq!(items.len(), 2);
        let first = items[0].as_text().unwrap_or_default();
        let second = items[1].as_text().unwrap_or_default();
        assert!(first.contains("[Notification]"));
        assert!(first.contains("first"));
        assert!(second.contains("[Notification]"));
        assert!(second.contains("second"));
        assert!(
            buffer.is_empty(),
            "buffer must be drained after pending_history_appends"
        );

        // Empty buffer → empty Vec (no synthesised items).
        let again = interceptor.pending_history_appends().await.unwrap();
        assert!(again.is_empty());
    }

    #[tokio::test]
    async fn pre_llm_request_does_not_touch_pending_notifies() {
        // The drain lane has moved to `pending_history_appends`;
        // `pre_llm_request` must leave the buffer alone and not inject
        // anything itself.
        let registry = Arc::new(HookRegistryBuilder::new().build());
        let buffer = NotifyBuffer::new();
        buffer.push_notify("msg".into(), false);

        let interceptor = WorkerInterceptor::new(
            registry,
            None,
            None,
            buffer.clone(),
            Arc::new(Mutex::new(Vec::new())),
            PromptCatalog::builtins_only().unwrap(),
            None,
        );
        let mut ctx: Vec<Item> = vec![Item::user_message("hi")];
        let action = interceptor.pre_llm_request(&mut ctx).await;

        assert!(matches!(action, PreRequestAction::Continue));
        assert_eq!(ctx.len(), 1, "pre_llm_request must not append notifies");
        assert_eq!(
            buffer.len(),
            1,
            "pre_llm_request must not drain the notify buffer"
        );
    }

    #[tokio::test]
    async fn pre_llm_request_short_circuits_on_first_non_continue() {
        let first_called = Arc::new(AtomicBool::new(false));
        let second_count = Arc::new(AtomicUsize::new(0));
        let mut builder = HookRegistryBuilder::new();
        builder.add_pre_llm_request(AbortingHook(first_called.clone()));
        builder.add_pre_llm_request(CountingHook(second_count.clone()));
        let registry = Arc::new(builder.build());

        let interceptor = WorkerInterceptor::new(
            registry,
            None,
            None,
            NotifyBuffer::new(),
            Arc::new(Mutex::new(Vec::new())),
            PromptCatalog::builtins_only().unwrap(),
            None,
        );
        let mut ctx: Vec<Item> = Vec::new();
        let action = interceptor.pre_llm_request(&mut ctx).await;

        assert!(matches!(action, PreRequestAction::Cancel(_)));
        assert!(first_called.load(Ordering::Relaxed));
        assert_eq!(second_count.load(Ordering::Relaxed), 0);
    }
}
