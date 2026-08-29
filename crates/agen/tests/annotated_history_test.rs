mod common;

use agen::llm_client::event::{Event, ResponseStatus, StatusEvent};
use agen::{Engine, EngineError, History, HistoryEntry, Item, Role};
use common::MockLlmClient;

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
