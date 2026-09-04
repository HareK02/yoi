mod common;

use agen::interceptor::{
    AssistantTurnEndContext, Interceptor, InterceptorCallId, InterceptorInvocation,
    InterceptorPhase, InterceptorResult, PendingHistoryAppendsContext, PreLlmRequestContext,
    PreRequestAction, PromptAction, PromptSubmitContext, RunExitContext, TurnEndAction,
};
use agen::llm_client::event::{Event, ResponseStatus, StatusEvent};
use agen::{Engine, EngineError, History, HistoryEntry, Item, Role};
use async_trait::async_trait;
use common::MockLlmClient;
use std::sync::{Arc, Mutex};

fn completed_text_events(text: &str) -> Vec<Event> {
    vec![
        Event::text_block_start(0),
        Event::text_delta(0, text),
        Event::text_block_stop(0, None),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]
}

#[tokio::test]
async fn run_preserves_item_annotations_without_projecting_them() {
    let client = MockLlmClient::new(completed_text_events("assistant reply"));
    let engine = Engine::<_, agen::state::Mutable, String>::new_annotated(client);
    let mut history = History::<String>::new();
    let mut next = 0usize;
    let mut annotate = |item: &Item| {
        next += 1;
        let kind = match item {
            Item::Message { role, .. } => match role {
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::System => "system",
            },
            Item::ToolCall { .. } => "tool_call",
            Item::ToolResult { .. } => "tool_result",
            Item::Reasoning { .. } => "reasoning",
        };
        Ok(format!("{next}:{kind}"))
    };

    let output = engine
        .run_with_annotation(&mut history, "hello", &mut annotate)
        .await;

    assert!(matches!(output.result, agen::EngineRunExit::Finished));
    assert_eq!(history.len(), 2);
    assert_eq!(history.entries()[0].annotation, "1:user");
    assert_eq!(history.entries()[1].annotation, "2:assistant");
    assert_eq!(history.items_cloned().len(), 2);
}

#[derive(Clone)]
struct AnnotationObservingInterceptor {
    observed: Arc<Mutex<Vec<(InterceptorInvocation, Vec<String>)>>>,
}

impl AnnotationObservingInterceptor {
    fn record(&self, invocation: &InterceptorInvocation, history: &[HistoryEntry<String>]) {
        self.observed.lock().unwrap().push((
            invocation.clone(),
            history
                .iter()
                .map(|entry| entry.annotation.clone())
                .collect(),
        ));
    }
}

#[async_trait]
impl Interceptor<String> for AnnotationObservingInterceptor {
    async fn on_prompt_submit(
        &self,
        context: PromptSubmitContext<'_, String>,
    ) -> InterceptorResult<PromptAction> {
        self.record(&context.invocation, context.history);
        Ok(PromptAction::Continue)
    }

    async fn pending_history_appends(
        &self,
        context: PendingHistoryAppendsContext<'_, String>,
    ) -> InterceptorResult<Vec<Item>> {
        self.record(&context.invocation, context.history);
        Ok(Vec::new())
    }

    async fn pre_llm_request(
        &self,
        context: PreLlmRequestContext<'_, String>,
    ) -> InterceptorResult<PreRequestAction> {
        self.record(&context.invocation, context.history);
        Ok(PreRequestAction::Continue)
    }

    async fn on_assistant_turn_end(
        &self,
        context: AssistantTurnEndContext<'_, String>,
    ) -> InterceptorResult<TurnEndAction> {
        assert_eq!(context.assistant_entries.len(), 1);
        assert_eq!(context.assistant_entries[0].annotation, "2:assistant");
        self.record(&context.invocation, context.history);
        Ok(TurnEndAction::Finish)
    }

    async fn on_run_exit(&self, context: RunExitContext<'_, String>) -> InterceptorResult<()> {
        self.record(&context.invocation, context.history);
        Ok(())
    }
}

#[tokio::test]
async fn interceptor_contexts_preserve_annotations_and_typed_lifecycle_identity() {
    let client = MockLlmClient::new(completed_text_events("assistant reply"));
    let mut engine = Engine::<_, agen::state::Mutable, String>::new_annotated(client);
    let observed = Arc::new(Mutex::new(Vec::new()));
    engine.set_interceptor(AnnotationObservingInterceptor {
        observed: observed.clone(),
    });
    let mut history = History::<String>::new();
    let mut next = 0usize;
    let mut annotate = |item: &Item| {
        next += 1;
        let kind = if item.is_assistant_message() {
            "assistant"
        } else {
            "user"
        };
        Ok(format!("{next}:{kind}"))
    };

    let output = engine
        .run_with_annotation(&mut history, "hello", &mut annotate)
        .await;
    assert!(matches!(output.result, agen::EngineRunExit::Finished));

    let observed = observed.lock().unwrap();
    let phases: Vec<_> = observed
        .iter()
        .map(|(invocation, _)| invocation.phase)
        .collect();
    assert_eq!(
        phases,
        [
            InterceptorPhase::PromptSubmit,
            InterceptorPhase::PendingHistoryAppends,
            InterceptorPhase::PreLlmRequest,
            InterceptorPhase::AssistantTurnEnd,
            InterceptorPhase::RunExit,
        ]
    );
    assert!(
        observed
            .iter()
            .all(|(invocation, _)| invocation.run_id == observed[0].0.run_id)
    );
    assert_eq!(
        observed
            .iter()
            .map(|(invocation, _)| invocation.counters.invocation.get())
            .collect::<Vec<_>>(),
        [0, 1, 2, 3, 4]
    );
    assert_eq!(observed[2].0.call_id, Some(InterceptorCallId::Llm(0)));
    assert_eq!(observed[3].0.call_id, Some(InterceptorCallId::Llm(0)));
    assert_eq!(observed[1].1, ["1:user"]);
    assert_eq!(observed[2].1, ["1:user"]);
    assert_eq!(observed[3].1, ["1:user", "2:assistant"]);
    assert_eq!(observed[4].1, ["1:user", "2:assistant"]);
}

#[test]
fn append_failure_does_not_make_item_live() {
    let client = MockLlmClient::new(vec![]);
    let mut engine = Engine::<_, agen::state::Mutable, usize>::new_annotated(client);
    let mut history = History::<usize>::new();
    let mut fail = |_item: &Item| Err("commit failed".to_string());

    let err = engine
        .append_history_with(&mut history, [Item::user_message("uncommitted")], &mut fail)
        .unwrap_err();

    assert!(matches!(err, EngineError::HistoryAppend(message) if message == "commit failed"));
    assert!(history.is_empty());
}

#[test]
fn replacement_keeps_items_and_annotations_together() {
    let mut history = History::from_entries(vec![
        HistoryEntry::new(Item::user_message("old"), "old-ann".to_string()),
        HistoryEntry::new(Item::user_message("second"), "second-ann".to_string()),
    ]);

    history.truncate(1);
    assert_eq!(history.entries()[0].item.as_text(), Some("old"));
    assert_eq!(history.entries()[0].annotation, "old-ann");

    let previous = history.replace_entries(vec![HistoryEntry::new(
        Item::user_message("restored"),
        "restored-ann".to_string(),
    )]);

    assert_eq!(previous.len(), 1);
    assert_eq!(history.entries()[0].item.as_text(), Some("restored"));
    assert_eq!(history.entries()[0].annotation, "restored-ann");
}
