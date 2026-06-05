//! Task tools built-in feature module.
//!
//! The built-in Task feature owns the session-lifetime [`tools::TaskStore`]
//! shared by the Task tools and reminder hooks. Pod hosts install this module
//! through the feature contribution boundary and use its narrow snapshot surface
//! for restore/rewind/compaction compatibility; Pod does not own Task-specific
//! store or reminder state.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use llm_worker::Item;

use crate::feature::{
    FeatureDescriptor, FeatureHookPoint, FeatureInstallContext, FeatureInstallError, FeatureModule,
    HookDeclaration, ToolContribution, ToolDeclaration,
};
use crate::hook::{
    Hook, HookPreRequestAction, HookPreToolAction, PreLlmRequest, PreRequestContext, PreToolCall,
    ToolCallSummary,
};

const TASK_REMINDER_REQUEST_THRESHOLD: usize = 24;
const TASK_REMINDER_COOLDOWN_REQUESTS: usize = 24;
const TASK_MANAGEMENT_TOOL_NAMES: [&str; 2] = ["TaskCreate", "TaskUpdate"];

/// Construct the built-in Task feature module with a fresh session store.
///
/// The returned module contributes `TaskCreate`, `TaskUpdate`, `TaskGet`, and
/// `TaskList` through descriptor-approved tool registration, plus built-in hooks
/// that maintain Task-reminder state. It does not request sandbox/external-plugin
/// host authorities; normal ToolRegistry and PreToolCall permission policy still
/// applies at call time.
pub fn task_tools_feature() -> TaskFeature {
    TaskFeature::new()
}

/// Built-in Task feature state and contribution module.
#[derive(Clone, Debug)]
pub struct TaskFeature {
    state: Arc<TaskFeatureState>,
}

#[derive(Debug)]
struct TaskFeatureState {
    task_store: tools::TaskStore,
    reminder_state: TaskReminderState,
}

impl TaskFeature {
    pub fn new() -> Self {
        Self::from_store(tools::TaskStore::new())
    }

    pub fn from_history(history: &[Item]) -> Self {
        Self::from_store(tools::TaskStore::from_history(history))
    }

    fn from_store(task_store: tools::TaskStore) -> Self {
        Self {
            state: Arc::new(TaskFeatureState {
                task_store,
                reminder_state: TaskReminderState::new(),
            }),
        }
    }

    /// Restore the feature-owned store by replaying durable history into the
    /// existing shared store handle. Existing Task tool instances and hooks keep
    /// pointing at the same feature-owned store after rewind.
    pub fn restore_from_history(&self, history: &[Item]) {
        let restored = tools::TaskStore::from_history(history);
        self.state.task_store.replace_with(restored.list());
    }

    /// Feature-owned snapshot text used by compaction to preserve Task state.
    pub fn snapshot_text(&self) -> String {
        self.state.task_store.snapshot_text()
    }

    /// Feature-owned compact summary used for the synthetic TaskList result.
    pub fn snapshot_overview(&self) -> String {
        tools::task::snapshot_overview(&self.state.task_store.list())
    }

    #[cfg(test)]
    fn task_store(&self) -> tools::TaskStore {
        self.state.task_store.clone()
    }
}

impl Default for TaskFeature {
    fn default() -> Self {
        Self::new()
    }
}

impl FeatureModule for TaskFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("task-tools", "Task tools")
            .with_description("Session-lifetime task tracking builtin tools")
            .with_tool(ToolDeclaration::new(
                "TaskCreate",
                "Create a session-lifetime user-visible task",
            ))
            .with_tool(ToolDeclaration::new(
                "TaskUpdate",
                "Update a session-lifetime user-visible task",
            ))
            .with_tool(ToolDeclaration::new(
                "TaskGet",
                "Get one session-lifetime user-visible task",
            ))
            .with_tool(ToolDeclaration::new(
                "TaskList",
                "List session-lifetime user-visible tasks",
            ))
            .with_hook(HookDeclaration::new(
                "task-reminder-pre-request",
                FeatureHookPoint::PreRequest,
            ))
            .with_hook(HookDeclaration::new(
                "task-reminder-tool-usage",
                FeatureHookPoint::PreToolCall,
            ))
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        let names = ["TaskCreate", "TaskList", "TaskGet", "TaskUpdate"];
        for (name, definition) in names
            .into_iter()
            .zip(tools::task_tools(self.state.task_store.clone()))
        {
            context
                .tools()
                .register(ToolContribution::new(name, definition))?;
        }

        context.hooks().add_pre_request(
            "task-reminder-pre-request",
            TaskReminderPreRequestHook {
                state: Arc::clone(&self.state),
            },
        )?;
        context.hooks().add_pre_tool_call(
            "task-reminder-tool-usage",
            TaskReminderToolUsageHook {
                state: Arc::clone(&self.state),
            },
        )?;
        Ok(())
    }
}

#[derive(Debug)]
struct TaskReminderState {
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
    fn new() -> Self {
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

struct TaskReminderPreRequestHook {
    state: Arc<TaskFeatureState>,
}

#[async_trait]
impl Hook<PreLlmRequest> for TaskReminderPreRequestHook {
    async fn call(&self, input: &PreRequestContext) -> HookPreRequestAction {
        let active_tasks: Vec<tools::TaskEntry> = self
            .state
            .task_store
            .list()
            .into_iter()
            .filter(|task| {
                matches!(
                    task.status,
                    tools::TaskStatus::Pending | tools::TaskStatus::Inprogress
                )
            })
            .collect();
        if active_tasks.is_empty() {
            return HookPreRequestAction::Continue;
        }

        let (since_task_management, since_reminder) = self.state.reminder_state.note_request();
        if since_task_management < TASK_REMINDER_REQUEST_THRESHOLD
            || since_reminder < TASK_REMINDER_COOLDOWN_REQUESTS
        {
            return HookPreRequestAction::Continue;
        }

        if let Some(system_items) = input.system_items() {
            self.state.reminder_state.note_reminder();
            system_items.append_task_reminder(render_task_reminder_body(&active_tasks));
        }
        HookPreRequestAction::Continue
    }
}

struct TaskReminderToolUsageHook {
    state: Arc<TaskFeatureState>,
}

#[async_trait]
impl Hook<PreToolCall> for TaskReminderToolUsageHook {
    async fn call(&self, input: &ToolCallSummary) -> HookPreToolAction {
        if is_task_management_tool(&input.tool_name) {
            self.state.reminder_state.note_task_management();
        }
        HookPreToolAction::Continue
    }
}

fn is_task_management_tool(name: &str) -> bool {
    TASK_MANAGEMENT_TOOL_NAMES.contains(&name)
}

fn render_task_reminder_body(active_tasks: &[tools::TaskEntry]) -> String {
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

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use session_store::{SystemItem, SystemReminderSource};

    use super::*;
    use crate::hook::{PreRequestInfo, SystemItemAppendHandle};

    fn pre_request_context(pending: Arc<Mutex<Vec<SystemItem>>>) -> PreRequestContext {
        PreRequestContext::new(
            PreRequestInfo {
                item_count: 1,
                estimated_tokens: None,
                turn_index: 0,
                tool_calls_this_turn: 0,
            },
            Some(SystemItemAppendHandle::new(pending)),
        )
    }

    fn tool_summary(name: &str) -> ToolCallSummary {
        ToolCallSummary {
            call_id: "call-id".into(),
            tool_name: name.into(),
            arguments: serde_json::json!({}),
        }
    }

    #[tokio::test]
    async fn task_reminder_hook_appends_after_inactive_request_threshold() {
        let feature = TaskFeature::new();
        feature
            .task_store()
            .create("keep going".into(), "long task description".into());
        let hook = TaskReminderPreRequestHook {
            state: Arc::clone(&feature.state),
        };
        let pending = Arc::new(Mutex::new(Vec::new()));

        for _ in 0..TASK_REMINDER_REQUEST_THRESHOLD - 1 {
            let _ = hook.call(&pre_request_context(Arc::clone(&pending))).await;
            assert!(pending.lock().expect("pending queue poisoned").is_empty());
        }
        let _ = hook.call(&pre_request_context(Arc::clone(&pending))).await;

        let queued = pending.lock().expect("pending queue poisoned");
        assert_eq!(queued.len(), 1);
        let SystemItem::TaskReminder { body, .. } = &queued[0] else {
            panic!("unexpected system item: {:?}", queued[0]);
        };
        assert_eq!(body.matches("<system-reminder>").count(), 1);
        assert_eq!(body.matches("</system-reminder>").count(), 1);
        assert!(body.contains("taskid 1"));
        assert!(body.contains("pending"));
        assert!(body.contains("keep going"));
        assert!(!body.contains("long task description"));
    }

    #[tokio::test]
    async fn task_reminder_hook_retains_source() {
        let feature = TaskFeature::new();
        feature.task_store().create("typed".into(), String::new());
        let hook = TaskReminderPreRequestHook {
            state: Arc::clone(&feature.state),
        };
        let pending = Arc::new(Mutex::new(Vec::new()));

        for _ in 0..TASK_REMINDER_REQUEST_THRESHOLD {
            let _ = hook.call(&pre_request_context(Arc::clone(&pending))).await;
        }

        let queued = pending.lock().expect("pending queue poisoned");
        let SystemItem::TaskReminder { source, body } = &queued[0] else {
            panic!("unexpected system item: {:?}", queued[0]);
        };
        assert_eq!(*source, SystemReminderSource::TaskInactivity);
        assert_eq!(body.matches("<system-reminder>").count(), 1);
        assert_eq!(body.matches("</system-reminder>").count(), 1);
        assert!(body.contains("typed"));
    }

    #[test]
    fn render_task_reminder_body_is_unwrapped_for_system_reminder_helper() {
        let feature = TaskFeature::new();
        let task = feature.task_store().create("body".into(), String::new());
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
        let feature = TaskFeature::new();
        feature
            .task_store()
            .create("track me".into(), String::new());
        let pre_request = TaskReminderPreRequestHook {
            state: Arc::clone(&feature.state),
        };
        let pre_tool = TaskReminderToolUsageHook {
            state: Arc::clone(&feature.state),
        };
        let pending = Arc::new(Mutex::new(Vec::new()));

        for _ in 0..TASK_REMINDER_REQUEST_THRESHOLD - 1 {
            let _ = pre_request
                .call(&pre_request_context(Arc::clone(&pending)))
                .await;
            assert!(pending.lock().expect("pending queue poisoned").is_empty());
        }
        let _ = pre_tool.call(&tool_summary("TaskUpdate")).await;

        for _ in 0..TASK_REMINDER_REQUEST_THRESHOLD - 1 {
            let _ = pre_request
                .call(&pre_request_context(Arc::clone(&pending)))
                .await;
            assert!(pending.lock().expect("pending queue poisoned").is_empty());
        }
        let _ = pre_request
            .call(&pre_request_context(Arc::clone(&pending)))
            .await;
        assert_eq!(pending.lock().expect("pending queue poisoned").len(), 1);
    }

    #[tokio::test]
    async fn task_reminder_respects_cooldown_after_reminder() {
        let feature = TaskFeature::new();
        feature
            .task_store()
            .create("cooldown".into(), String::new());
        let hook = TaskReminderPreRequestHook {
            state: Arc::clone(&feature.state),
        };
        let pending = Arc::new(Mutex::new(Vec::new()));

        for _ in 0..TASK_REMINDER_REQUEST_THRESHOLD {
            let _ = hook.call(&pre_request_context(Arc::clone(&pending))).await;
        }
        pending.lock().expect("pending queue poisoned").clear();
        for _ in 0..TASK_REMINDER_COOLDOWN_REQUESTS - 1 {
            let _ = hook.call(&pre_request_context(Arc::clone(&pending))).await;
            assert!(pending.lock().expect("pending queue poisoned").is_empty());
        }
        let _ = hook.call(&pre_request_context(Arc::clone(&pending))).await;
        assert_eq!(pending.lock().expect("pending queue poisoned").len(), 1);
    }

    #[tokio::test]
    async fn task_reminder_is_silent_when_no_active_tasks_exist() {
        let feature = TaskFeature::new();
        let done = feature
            .task_store()
            .create("done".into(), String::new())
            .taskid;
        feature
            .task_store()
            .update(done, Some(tools::TaskStatus::Completed), None, None)
            .expect("complete task");
        let hook = TaskReminderPreRequestHook {
            state: Arc::clone(&feature.state),
        };
        let pending = Arc::new(Mutex::new(Vec::new()));

        for _ in 0..TASK_REMINDER_REQUEST_THRESHOLD * 2 {
            let _ = hook.call(&pre_request_context(Arc::clone(&pending))).await;
            assert!(pending.lock().expect("pending queue poisoned").is_empty());
        }
    }

    #[tokio::test]
    async fn inactive_requests_without_active_tasks_do_not_prime_task_reminder() {
        let feature = TaskFeature::new();
        let hook = TaskReminderPreRequestHook {
            state: Arc::clone(&feature.state),
        };
        let pending = Arc::new(Mutex::new(Vec::new()));

        for _ in 0..TASK_REMINDER_REQUEST_THRESHOLD * 2 {
            let _ = hook.call(&pre_request_context(Arc::clone(&pending))).await;
            assert!(pending.lock().expect("pending queue poisoned").is_empty());
        }

        feature
            .task_store()
            .create("new active".into(), String::new());
        for _ in 0..TASK_REMINDER_REQUEST_THRESHOLD - 1 {
            let _ = hook.call(&pre_request_context(Arc::clone(&pending))).await;
            assert!(pending.lock().expect("pending queue poisoned").is_empty());
        }
        let _ = hook.call(&pre_request_context(Arc::clone(&pending))).await;
        assert_eq!(pending.lock().expect("pending queue poisoned").len(), 1);
    }

    #[tokio::test]
    async fn task_create_reset_does_not_block_first_reminder_cooldown() {
        let feature = TaskFeature::new();
        let pre_request = TaskReminderPreRequestHook {
            state: Arc::clone(&feature.state),
        };
        let pre_tool = TaskReminderToolUsageHook {
            state: Arc::clone(&feature.state),
        };
        let pending = Arc::new(Mutex::new(Vec::new()));

        for _ in 0..TASK_REMINDER_REQUEST_THRESHOLD * 2 {
            let _ = pre_request
                .call(&pre_request_context(Arc::clone(&pending)))
                .await;
            assert!(pending.lock().expect("pending queue poisoned").is_empty());
        }

        let _ = pre_tool.call(&tool_summary("TaskCreate")).await;
        feature
            .task_store()
            .create("created after idle".into(), String::new());
        assert_eq!(
            feature
                .state
                .reminder_state
                .requests_since_last_reminder
                .load(Ordering::Relaxed),
            TASK_REMINDER_COOLDOWN_REQUESTS,
            "TaskCreate reset must not clear the initial reminder cooldown"
        );

        for _ in 0..TASK_REMINDER_REQUEST_THRESHOLD - 1 {
            let _ = pre_request
                .call(&pre_request_context(Arc::clone(&pending)))
                .await;
            assert!(pending.lock().expect("pending queue poisoned").is_empty());
        }
        let _ = pre_request
            .call(&pre_request_context(Arc::clone(&pending)))
            .await;
        assert_eq!(pending.lock().expect("pending queue poisoned").len(), 1);
    }

    #[tokio::test]
    async fn missing_system_item_handle_does_not_mark_reminder_sent() {
        let feature = TaskFeature::new();
        feature.task_store().create("handle".into(), String::new());
        let hook = TaskReminderPreRequestHook {
            state: Arc::clone(&feature.state),
        };
        let no_handle = PreRequestContext::new(
            PreRequestInfo {
                item_count: 1,
                estimated_tokens: None,
                turn_index: 0,
                tool_calls_this_turn: 0,
            },
            None,
        );

        for _ in 0..TASK_REMINDER_REQUEST_THRESHOLD {
            let _ = hook.call(&no_handle).await;
        }
        assert_eq!(
            feature
                .state
                .reminder_state
                .requests_since_last_reminder
                .load(Ordering::Relaxed),
            TASK_REMINDER_COOLDOWN_REQUESTS + TASK_REMINDER_REQUEST_THRESHOLD,
            "without a handle the hook must not record a reminder as emitted"
        );
    }

    #[test]
    fn restore_from_history_keeps_existing_store_handle_for_installed_tools() {
        let feature = TaskFeature::new();
        let handle = feature.task_store();
        handle.create("old".into(), String::new());
        let history = vec![Item::tool_call(
            "c1",
            "TaskCreate",
            r#"{"subject":"restored","description":"from history"}"#,
        )];

        feature.restore_from_history(&history);

        let tasks = handle.list();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].subject, "restored");
    }
}
