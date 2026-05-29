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
use llm_worker::UsageRecord;
use llm_worker::interceptor::{
    Interceptor, PostToolAction, PreRequestAction, PreToolAction, PromptAction, ToolCallInfo,
    ToolResultInfo, TurnEndAction,
};
use llm_worker::tool::ToolOutput;
use tracing::info;
use tracing::warn;

use crate::compact::state::CompactState;
use crate::compact::usage_tracker::UsageTracker;
use session_store::{SystemItem, SystemReminder};
use tools::{TaskEntry, TaskStatus, TaskStore};

use crate::hook::{
    AbortInfo, HookPromptAction, HookRegistry, PreRequestInfo, PromptSubmitInfo, ToolCallSummary,
    ToolResultSummary, TurnEndInfo,
};
use crate::ipc::notify_buffer::{NotifyBuffer, build_system_item};
use crate::pod::SystemItemCommitter;
use crate::prompt::catalog::PromptCatalog;
use llm_worker::token_counter::total_tokens;

/// Maximum number of bytes copied into `TurnEndInfo::final_text_preview`.
const FINAL_TEXT_PREVIEW_LIMIT: usize = 512;

const TASK_REMINDER_REQUEST_THRESHOLD: usize = 24;
const TASK_REMINDER_COOLDOWN_REQUESTS: usize = 24;
const TASK_MANAGEMENT_TOOL_NAMES: [&str; 2] = ["TaskCreate", "TaskUpdate"];

#[derive(Debug)]
pub(crate) struct TaskReminderState {
    requests_since_last_task_management: AtomicUsize,
    requests_since_last_reminder: AtomicUsize,
}

impl Default for TaskReminderState {
    fn default() -> Self {
        Self {
            requests_since_last_task_management: AtomicUsize::new(0),
            requests_since_last_reminder: AtomicUsize::new(TASK_REMINDER_COOLDOWN_REQUESTS),
        }
    }
}

impl TaskReminderState {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn note_request(&self) -> (usize, usize) {
        let since_task_management = self
            .requests_since_last_task_management
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        let since_reminder = self
            .requests_since_last_reminder
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        (since_task_management, since_reminder)
    }

    fn note_task_management(&self) {
        self.requests_since_last_task_management
            .store(0, Ordering::Relaxed);
    }

    fn note_reminder(&self) {
        self.requests_since_last_reminder
            .store(0, Ordering::Relaxed);
    }
}

pub(crate) struct PodInterceptor {
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
    /// request. The Worker `extend`s these into its persistent history
    /// so the LLM has a visible trigger for any reaction it commits.
    pending_notifies: NotifyBuffer,
    /// Submit-scoped stash of resolver-produced typed system items.
    /// Drained inside `on_prompt_submit`, committed as
    /// `LogEntry::SystemItem` entries through `log_writer`, and
    /// returned to the worker as `Item::system_message` via
    /// `PromptAction::ContinueWith`. Populated by `Pod::run`
    /// immediately before handing off to the worker.
    pending_attachments: Arc<Mutex<Vec<SystemItem>>>,
    /// Task state observed by built-in task tools. Used to nudge the main
    /// worker when active tasks have gone unmentioned for several requests.
    task_store: TaskStore,
    task_reminder_state: Arc<TaskReminderState>,
    /// Prompt catalog used to render pending notification entries into the
    /// same system-message text that will be persisted in history.
    prompts: Arc<PromptCatalog>,
    /// Type-erased commit handle. The interceptor uses it to commit
    /// `LogEntry::SystemItem` entries directly (sync) before
    /// returning the corresponding `Item::system_message`s up to the
    /// worker. `None` in tests / `Pod::new` paths where no writer is
    /// attached.
    log_writer: Option<Arc<dyn SystemItemCommitter>>,
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
        pending_attachments: Arc<Mutex<Vec<SystemItem>>>,
        task_store: TaskStore,
        task_reminder_state: Arc<TaskReminderState>,
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
            task_store,
            task_reminder_state,
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
    fn commit_system_items(&self, items: &[SystemItem]) {
        let Some(writer) = self.log_writer.as_ref() else {
            return;
        };
        for item in items {
            writer.commit_system_item(item.clone());
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
        let mut records = handle.lock().expect("usage_history poisoned").clone();
        if let Some(tracker) = self.usage_tracker.as_ref() {
            records.extend(tracker.records());
        }
        Some(total_tokens(context, &records).tokens)
    }

    fn task_reminder_system_item(&self) -> Option<SystemItem> {
        let active_tasks: Vec<TaskEntry> = self
            .task_store
            .list()
            .into_iter()
            .filter(|task| matches!(task.status, TaskStatus::Pending | TaskStatus::Inprogress))
            .collect();
        if active_tasks.is_empty() {
            return None;
        }

        let (since_task_management, since_reminder) = self.task_reminder_state.note_request();
        if since_task_management < TASK_REMINDER_REQUEST_THRESHOLD
            || since_reminder < TASK_REMINDER_COOLDOWN_REQUESTS
        {
            return None;
        }

        self.task_reminder_state.note_reminder();
        Some(
            SystemReminder::task_inactivity(render_task_reminder_body(&active_tasks))
                .into_system_item(),
        )
    }
}

fn is_task_management_tool(name: &str) -> bool {
    TASK_MANAGEMENT_TOOL_NAMES.contains(&name)
}

fn render_task_reminder_body(active_tasks: &[TaskEntry]) -> String {
    let mut body = String::from(
        "Active session tasks are still open. If progress changed, call TaskUpdate.\n",
    );
    for task in active_tasks {
        body.push_str(&format!(
            "- taskid {} ({}) {}\n",
            task.taskid, task.status, task.subject
        ));
    }
    body.trim_end_matches('\n').to_string()
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
            self.commit_system_items(&extras);
            PromptAction::ContinueWith(items)
        }
    }

    async fn pending_history_appends(&self) -> Vec<Item> {
        let drained = self.pending_notifies.drain();
        let task_reminder = self.task_reminder_system_item();
        if drained.is_empty() && task_reminder.is_none() {
            return Vec::new();
        }

        let mut system_items: Vec<SystemItem> = Vec::with_capacity(drained.len() + 1);
        let mut items: Vec<Item> = Vec::with_capacity(drained.len() + 1);
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
                        super::notify_buffer::PendingNotify::Notify { message } => message.clone(),
                        super::notify_buffer::PendingNotify::PodEvent { event } => {
                            session_store::render_pod_event(event)
                        }
                    };
                    items.push(Item::system_message(fallback));
                }
            }
        }
        if let Some(system_item) = task_reminder {
            items.push(system_item.to_history_item());
            system_items.push(system_item);
        }
        self.commit_system_items(&system_items);
        items
    }

    async fn pre_llm_request(&self, context: &mut Vec<Item>) -> PreRequestAction {
        let current_tokens = self.estimated_tokens(context);

        // Internal mechanism: between-requests compaction trigger (safety net).
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
                    return PreRequestAction::Yield;
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
        if is_task_management_tool(&info.call.name) {
            self.task_reminder_state.note_task_management();
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
    use crate::hook::{Hook, HookRegistryBuilder, PreLlmRequest};
    use session_store::SystemReminderSource;

    struct CountingHook(Arc<AtomicUsize>);

    #[async_trait]
    impl Hook<PreLlmRequest> for CountingHook {
        async fn call(&self, _info: &PreRequestInfo) -> PreRequestAction {
            self.0.fetch_add(1, Ordering::Relaxed);
            PreRequestAction::Continue
        }
    }

    fn registry_with_pre_llm_hook(count: Arc<AtomicUsize>) -> Arc<HookRegistry> {
        let mut builder = HookRegistryBuilder::new();
        builder.add_pre_llm_request(CountingHook(count));
        Arc::new(builder.build())
    }

    fn interceptor_for_task_reminders(
        task_store: TaskStore,
        task_reminder_state: Arc<TaskReminderState>,
    ) -> PodInterceptor {
        PodInterceptor::new(
            Arc::new(HookRegistryBuilder::new().build()),
            None,
            None,
            NotifyBuffer::new(),
            Arc::new(Mutex::new(Vec::new())),
            task_store,
            task_reminder_state,
            PromptCatalog::builtins_only().unwrap(),
            None,
        )
    }

    async fn call_pre_tool(interceptor: &PodInterceptor, name: &str) {
        let def = tools::task_tools(TaskStore::new())
            .into_iter()
            .find(|def| {
                let (meta, _) = def();
                meta.name == name
            })
            .expect("task tool definition");
        let (meta, tool) = def();
        let mut info = ToolCallInfo {
            call: llm_worker::tool::ToolCall {
                id: "call-id".into(),
                name: name.into(),
                input: serde_json::json!({}),
            },
            meta,
            tool,
        };
        let action = interceptor.pre_tool_call(&mut info).await;
        assert!(matches!(action, PreToolAction::Continue));
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

        let interceptor = PodInterceptor::new(
            registry,
            Some(state),
            Some(history),
            NotifyBuffer::new(),
            Arc::new(Mutex::new(Vec::new())),
            TaskStore::new(),
            Arc::new(TaskReminderState::new()),
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
    async fn pre_llm_request_counts_in_flight_usage_records() {
        let registry = Arc::new(HookRegistryBuilder::new().build());
        let state = Arc::new(CompactState::new(None, Some(100), 2));
        let ctx_items = vec![Item::user_message("hi")];
        let history = usage_handle_with(ctx_items.len(), 50);
        let usage_tracker = Arc::new(UsageTracker::new());
        usage_tracker.note_request(ctx_items.len());
        usage_tracker.record_usage(&llm_worker::event::UsageEvent {
            input_tokens: Some(150),
            output_tokens: Some(0),
            total_tokens: Some(150),
            cache_read_input_tokens: Some(0),
            cache_creation_input_tokens: Some(0),
        });

        let interceptor = PodInterceptor::new(
            registry,
            Some(state),
            Some(history),
            NotifyBuffer::new(),
            Arc::new(Mutex::new(Vec::new())),
            TaskStore::new(),
            Arc::new(TaskReminderState::new()),
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

        let interceptor = PodInterceptor::new(
            registry,
            Some(state),
            Some(history),
            NotifyBuffer::new(),
            Arc::new(Mutex::new(Vec::new())),
            TaskStore::new(),
            Arc::new(TaskReminderState::new()),
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

        let interceptor = PodInterceptor::new(
            registry,
            Some(state),
            Some(history),
            NotifyBuffer::new(),
            Arc::new(Mutex::new(Vec::new())),
            TaskStore::new(),
            Arc::new(TaskReminderState::new()),
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

        let interceptor = PodInterceptor::new(
            registry,
            None,
            None,
            NotifyBuffer::new(),
            Arc::new(Mutex::new(Vec::new())),
            TaskStore::new(),
            Arc::new(TaskReminderState::new()),
            PromptCatalog::builtins_only().unwrap(),
            None,
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
    async fn pending_history_appends_drains_buffer_into_items() {
        let registry = Arc::new(HookRegistryBuilder::new().build());
        let buffer = NotifyBuffer::new();
        buffer.push_notify("first".into());
        buffer.push_notify("second".into());

        let interceptor = PodInterceptor::new(
            registry,
            None,
            None,
            buffer.clone(),
            Arc::new(Mutex::new(Vec::new())),
            TaskStore::new(),
            Arc::new(TaskReminderState::new()),
            PromptCatalog::builtins_only().unwrap(),
            None,
        );

        let items = interceptor.pending_history_appends().await;
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
        let again = interceptor.pending_history_appends().await;
        assert!(again.is_empty());
    }

    #[tokio::test]
    async fn task_reminder_appends_after_inactive_request_threshold() {
        let task_store = TaskStore::new();
        task_store.create("keep going".into(), "long task description".into());
        let interceptor =
            interceptor_for_task_reminders(task_store, Arc::new(TaskReminderState::new()));

        for _ in 0..TASK_REMINDER_REQUEST_THRESHOLD - 1 {
            assert!(interceptor.pending_history_appends().await.is_empty());
        }
        let items = interceptor.pending_history_appends().await;
        assert_eq!(items.len(), 1);
        let body = items[0].as_text().unwrap_or_default();
        assert_eq!(body.matches("<system-reminder>").count(), 1);
        assert_eq!(body.matches("</system-reminder>").count(), 1);
        assert!(body.contains("taskid 1"));
        assert!(body.contains("pending"));
        assert!(body.contains("keep going"));
        assert!(!body.contains("long task description"));
    }

    #[test]
    fn task_reminder_system_item_retains_source() {
        let task_store = TaskStore::new();
        task_store.create("typed".into(), String::new());
        let interceptor =
            interceptor_for_task_reminders(task_store, Arc::new(TaskReminderState::new()));

        for _ in 0..TASK_REMINDER_REQUEST_THRESHOLD - 1 {
            assert!(interceptor.task_reminder_system_item().is_none());
        }
        let item = interceptor.task_reminder_system_item().unwrap();
        match item {
            SystemItem::TaskReminder { source, body } => {
                assert_eq!(source, SystemReminderSource::TaskInactivity);
                assert_eq!(body.matches("<system-reminder>").count(), 1);
                assert_eq!(body.matches("</system-reminder>").count(), 1);
                assert!(body.contains("typed"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn render_task_reminder_body_is_unwrapped_for_system_reminder_helper() {
        let task_store = TaskStore::new();
        let task = task_store.create("body".into(), String::new());
        let body = render_task_reminder_body(&[task]);

        assert!(!body.contains("<system-reminder>"));
        assert!(!body.contains("</system-reminder>"));
        assert!(body.contains("TaskUpdate"));
        assert!(body.contains("taskid 1"));
    }

    #[test]
    fn task_reminder_state_starts_with_initial_cooldown_elapsed() {
        let state = TaskReminderState::new();

        assert_eq!(
            state.requests_since_last_reminder.load(Ordering::Relaxed),
            TASK_REMINDER_COOLDOWN_REQUESTS
        );
        assert_eq!(
            state
                .requests_since_last_task_management
                .load(Ordering::Relaxed),
            0
        );
    }

    #[tokio::test]
    async fn task_management_tool_call_resets_reminder_inactivity_counter() {
        let task_store = TaskStore::new();
        task_store.create("track me".into(), String::new());
        let interceptor =
            interceptor_for_task_reminders(task_store, Arc::new(TaskReminderState::new()));

        for _ in 0..TASK_REMINDER_REQUEST_THRESHOLD - 1 {
            assert!(interceptor.pending_history_appends().await.is_empty());
        }
        call_pre_tool(&interceptor, "TaskUpdate").await;

        for _ in 0..TASK_REMINDER_REQUEST_THRESHOLD - 1 {
            assert!(interceptor.pending_history_appends().await.is_empty());
        }
        assert_eq!(interceptor.pending_history_appends().await.len(), 1);
    }

    #[tokio::test]
    async fn task_reminder_respects_cooldown_after_reminder() {
        let task_store = TaskStore::new();
        task_store.create("cooldown".into(), String::new());
        let interceptor =
            interceptor_for_task_reminders(task_store, Arc::new(TaskReminderState::new()));

        for _ in 0..TASK_REMINDER_REQUEST_THRESHOLD {
            let _ = interceptor.pending_history_appends().await;
        }
        for _ in 0..TASK_REMINDER_COOLDOWN_REQUESTS - 1 {
            assert!(interceptor.pending_history_appends().await.is_empty());
        }
        assert_eq!(interceptor.pending_history_appends().await.len(), 1);
    }

    #[tokio::test]
    async fn task_reminder_is_silent_when_no_active_tasks_exist() {
        let task_store = TaskStore::new();
        let done = task_store.create("done".into(), String::new()).taskid;
        task_store
            .update(done, Some(TaskStatus::Completed), None, None)
            .expect("complete task");
        let interceptor =
            interceptor_for_task_reminders(task_store, Arc::new(TaskReminderState::new()));

        for _ in 0..TASK_REMINDER_REQUEST_THRESHOLD * 2 {
            assert!(interceptor.pending_history_appends().await.is_empty());
        }
    }

    #[tokio::test]
    async fn inactive_requests_without_active_tasks_do_not_prime_task_reminder() {
        let task_store = TaskStore::new();
        let interceptor =
            interceptor_for_task_reminders(task_store.clone(), Arc::new(TaskReminderState::new()));

        for _ in 0..TASK_REMINDER_REQUEST_THRESHOLD * 2 {
            assert!(interceptor.pending_history_appends().await.is_empty());
        }

        task_store.create("new active".into(), String::new());
        for _ in 0..TASK_REMINDER_REQUEST_THRESHOLD - 1 {
            assert!(interceptor.pending_history_appends().await.is_empty());
        }
        assert_eq!(interceptor.pending_history_appends().await.len(), 1);
    }

    #[tokio::test]
    async fn task_create_reset_does_not_block_first_reminder_cooldown() {
        let task_store = TaskStore::new();
        let state = Arc::new(TaskReminderState::new());
        let interceptor = interceptor_for_task_reminders(task_store.clone(), state.clone());

        for _ in 0..TASK_REMINDER_REQUEST_THRESHOLD * 2 {
            assert!(interceptor.pending_history_appends().await.is_empty());
        }

        call_pre_tool(&interceptor, "TaskCreate").await;
        task_store.create("created after idle".into(), String::new());
        assert_eq!(
            state.requests_since_last_reminder.load(Ordering::Relaxed),
            TASK_REMINDER_COOLDOWN_REQUESTS,
            "TaskCreate reset must not clear the initial reminder cooldown"
        );

        for _ in 0..TASK_REMINDER_REQUEST_THRESHOLD - 1 {
            assert!(interceptor.pending_history_appends().await.is_empty());
        }
        assert_eq!(interceptor.pending_history_appends().await.len(), 1);
    }

    #[tokio::test]
    async fn task_reminder_lands_in_pending_history_appends_lane() {
        let task_store = TaskStore::new();
        task_store.create("lane".into(), String::new());
        let interceptor =
            interceptor_for_task_reminders(task_store, Arc::new(TaskReminderState::new()));
        let mut ctx = vec![Item::user_message("hi")];

        for _ in 0..TASK_REMINDER_REQUEST_THRESHOLD {
            let _ = interceptor.pending_history_appends().await;
        }
        let action = interceptor.pre_llm_request(&mut ctx).await;

        assert!(matches!(action, PreRequestAction::Continue));
        assert_eq!(ctx.len(), 1, "pre_llm_request must not inject reminders");
    }

    #[tokio::test]
    async fn pre_llm_request_does_not_touch_task_reminder_lane() {
        let task_store = TaskStore::new();
        task_store.create("lane".into(), String::new());
        let interceptor =
            interceptor_for_task_reminders(task_store, Arc::new(TaskReminderState::new()));
        let mut ctx = vec![Item::user_message("hi")];

        for _ in 0..TASK_REMINDER_REQUEST_THRESHOLD - 1 {
            assert!(interceptor.pending_history_appends().await.is_empty());
        }
        let action = interceptor.pre_llm_request(&mut ctx).await;

        assert!(matches!(action, PreRequestAction::Continue));
        assert_eq!(ctx.len(), 1, "pre_llm_request must not inject reminders");
        let pending = interceptor.pending_history_appends().await;
        assert_eq!(
            pending.len(),
            1,
            "reminders stay in pending_history_appends"
        );
    }

    #[tokio::test]
    async fn pre_llm_request_does_not_touch_pending_notifies() {
        // The drain lane has moved to `pending_history_appends`;
        // `pre_llm_request` must leave the buffer alone and not inject
        // anything itself.
        let registry = Arc::new(HookRegistryBuilder::new().build());
        let buffer = NotifyBuffer::new();
        buffer.push_notify("msg".into());

        let interceptor = PodInterceptor::new(
            registry,
            None,
            None,
            buffer.clone(),
            Arc::new(Mutex::new(Vec::new())),
            TaskStore::new(),
            Arc::new(TaskReminderState::new()),
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

        let interceptor = PodInterceptor::new(
            registry,
            None,
            None,
            NotifyBuffer::new(),
            Arc::new(Mutex::new(Vec::new())),
            TaskStore::new(),
            Arc::new(TaskReminderState::new()),
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
