//! Engine component tests with a recording tool and scripted host annotations.
//! Persistence is represented by the append callback; Server/IPC storage is not
//! exercised here. Run with `cargo test -p agen --lib tool_call_identity_tests`.

use super::*;
use async_trait::async_trait;
use std::sync::Mutex;

#[derive(Clone)]
struct NoLlm;

#[async_trait]
impl LlmClient for NoLlm {
    async fn stream(&self, _: Request) -> Result<ResponseStream, ClientError> {
        panic!("identity tests must not request a model response")
    }

    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
}

struct HostIdentity {
    early: bool,
}

#[async_trait]
impl Interceptor<String> for HostIdentity {
    fn logical_tool_call_id(&self, annotation: &String) -> Option<String> {
        Some(annotation.clone())
    }

    async fn pre_tool_call(
        &self,
        info: &mut ToolCallInfo<'_, String>,
    ) -> Result<PreToolAction, InterceptorError> {
        if self.early {
            assert!(info.history.is_empty());
        }
        // Pre-tool hooks may replace the context, but cannot replace persisted identity.
        info.context.logical_call_id = Some("hook-forged-identity".into());
        Ok(PreToolAction::Continue)
    }
}

struct RecordingTool(Arc<Mutex<Vec<ToolExecutionContext>>>);

#[async_trait]
impl crate::tool::Tool for RecordingTool {
    async fn execute(
        &self,
        _: &str,
        context: ToolExecutionContext,
    ) -> Result<crate::tool::ToolOutput, ToolError> {
        self.0.lock().unwrap().push(context);
        Ok("recorded".to_string().into())
    }
}

fn engine(
    early: bool,
) -> (
    Engine<NoLlm, Mutable, String>,
    Arc<Mutex<Vec<ToolExecutionContext>>>,
) {
    let mut engine = Engine::new_annotated(NoLlm);
    engine.set_interceptor(HostIdentity { early });
    engine.max_turns = Some(0);
    engine.start_logical_run();
    let observed = Arc::new(Mutex::new(Vec::new()));
    let tool = Arc::new(RecordingTool(observed.clone()));
    engine.register_tool(Arc::new(move || {
        (crate::tool::ToolMeta::new("record"), tool.clone())
    }));
    engine.tool_server_handle().flush_pending();
    (engine, observed)
}

fn entry(item: Item, id: &str) -> HistoryEntry<String> {
    HistoryEntry::new(item, id.to_string())
}

fn call(id: &str) -> Item {
    Item::tool_call_json(id, "record", json!({}))
}

#[test]
fn results_answer_only_preceding_occurrences_of_reused_provider_ids() {
    let history = History::from_entries(vec![
        entry(
            Item::tool_result("orphan", "no preceding call"),
            "orphan-result",
        ),
        entry(call("reused"), "host-first"),
        entry(call("sibling"), "host-sibling"),
        entry(
            Item::tool_result("sibling", "out of order"),
            "sibling-result",
        ),
        entry(Item::tool_result("reused", "answered"), "first-result"),
        entry(call("reused"), "host-pending"),
        entry(call("orphan"), "host-after-orphan"),
    ]);
    let pending = unanswered_tool_call_entries(&history);
    assert_eq!(
        pending
            .iter()
            .map(|entry| entry.annotation.as_str())
            .collect::<Vec<_>>(),
        ["host-pending", "host-after-orphan"]
    );
}

#[tokio::test]
async fn early_dispatch_binds_identity_only_after_successful_append() {
    for reject_append in [false, true] {
        let (mut engine, observed) = engine(true);
        let mut history = History::new();
        let mut batch = EarlyToolExecutionBatch::new("live-batch".into(), 0);
        let result = engine
            .admit_early_tool_calls(
                &mut history,
                &mut |_| {
                    if reject_append {
                        Err("storage unavailable".into())
                    } else {
                        Ok("host-committed-entry".into())
                    }
                },
                &mut batch,
                vec![crate::timeline::CollectedToolCall {
                    call_index: 0,
                    call: ToolCall {
                        id: "provider-call".into(),
                        name: "record".into(),
                        input: json!({}),
                    },
                }],
            )
            .await;
        if reject_append {
            assert!(matches!(result, Err(EngineError::HistoryAppend(_))));
            assert!(history.is_empty());
            assert!(batch.execution_handles.is_empty());
            assert!(observed.lock().unwrap().is_empty());
        } else {
            assert!(matches!(result, Ok(EarlyToolAdmission::Continue)));
            let (_, terminal) = batch.futures.next().await.unwrap();
            assert_eq!(terminal.tool_use_id, "provider-call");
            let observed = observed.lock().unwrap();
            assert_eq!(observed.len(), 1);
            assert_eq!(
                observed[0].logical_call_id.as_deref(),
                Some(history.entries()[0].annotation.as_str())
            );
            assert_eq!(
                observed[0].logical_call_id.as_deref(),
                Some("host-committed-entry")
            );
            assert_eq!(batch.call_info_map["provider-call"].3, observed[0]);
        }
    }
}

#[tokio::test]
async fn normal_dispatch_and_unstarted_restore_use_the_exact_unanswered_annotation() {
    for restore in [false, true] {
        let (mut engine, observed) = engine(false);
        let mut history = History::from_entries(vec![
            entry(call("reused"), "host-answered"),
            entry(
                Item::tool_result("reused", "already completed"),
                "answered-result",
            ),
        ]);
        engine
            .append_history_items(&mut history, [call("reused")], &mut |_| {
                Ok("host-pending".into())
            })
            .unwrap();
        if restore {
            let persisted = serde_json::to_string(&history).unwrap();
            history = serde_json::from_str(&persisted).unwrap();
        }
        let (pending, started) = engine.get_unanswered_tool_calls(&history);
        assert_eq!(pending.len(), 1);
        assert!(started.is_empty());
        if restore {
            engine
                .run_turn_loop(&mut history, &mut |_| Ok("terminal-result".into()))
                .await
                .unwrap();
        } else {
            let result = engine
                .execute_tools(&mut history, &mut |_| Ok("terminal-result".into()), pending)
                .await
                .unwrap();
            assert!(matches!(result, ToolExecutionResult::Completed));
        }
        assert_eq!(observed.lock().unwrap().len(), 1);
        let context = observed.lock().unwrap()[0].clone();
        assert_eq!(context.call_id, "reused");
        assert_eq!(context.logical_call_id.as_deref(), Some("host-pending"));
        // A subsequent resume sees the terminal result, not another live attempt.
        engine
            .run_turn_loop(&mut history, &mut |_| Ok("unexpected-append".into()))
            .await
            .unwrap();
        assert_eq!(observed.lock().unwrap().len(), 1);
        assert!(unanswered_tool_call_entries(&history).is_empty());
    }
}

#[tokio::test]
async fn started_restore_with_reused_provider_id_closes_unknown_without_reexecution() {
    let (mut engine, observed) = engine(false);
    let mut history = History::from_entries(vec![
        entry(call("reused"), "host-answered"),
        entry(
            Item::tool_result("reused", "already completed"),
            "answered-result",
        ),
        entry(
            call("reused")
                .with_tool_execution_metadata(0, Some("old-attempt".into()))
                .with_status(ItemStatus::InProgress),
            "host-started",
        ),
    ]);
    let (pending, started) = engine.get_unanswered_tool_calls(&history);
    assert!(pending.is_empty());
    assert_eq!(started, ["reused"]);
    engine
        .run_turn_loop(&mut history, &mut |_| Ok("unknown-result".into()))
        .await
        .unwrap();
    assert!(observed.lock().unwrap().is_empty());
    assert!(matches!(
        &history.entries().last().unwrap().item,
        Item::ToolResult { call_id, disposition: ToolResultDisposition::OutcomeUnknown, .. }
            if call_id == "reused"
    ));
    let len = history.len();
    engine
        .run_turn_loop(&mut history, &mut |_| Ok("unexpected-append".into()))
        .await
        .unwrap();
    assert_eq!(history.len(), len);
    assert!(observed.lock().unwrap().is_empty());
}
