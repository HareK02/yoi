//! Worker-owned failure history integration through public APIs.
//!
//! Entry: `Worker::run_text` with a registered Tool returning the production
//! `tools::failure` adapter's error and a production Runtime-control cause.
//! Agen dispatch, Worker annotation/log writing, `WorkerSessionStore` JSONL
//! persistence, store replay, and Anthropic request serialization are real.
//! Only model responses and the operation producing the public command result
//! are fixtures; this does not exercise Runtime transport, command admission,
//! Controller bootstrap, or full Worker restoration. Reopening means dropping
//! the Worker and opening the temporary filesystem store again, not a Serde
//! roundtrip. The next model request also checks the already-committed prefix.
//!
//! Target: `cargo test -p worker --test failure_results` (default features).
//! All dependencies are already worker dependencies/dev-dependencies, including
//! `error-contract`; Cargo's automatic integration-target discovery is sufficient.
//! No credentials, live services, process environment changes, or sleeps are used.

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agen::Engine;
use agen::llm_client::event::{Event as LlmEvent, ResponseStatus, StatusEvent};
use agen::llm_client::scheme::{Scheme, anthropic::AnthropicScheme};
use agen::llm_client::{ClientError, LlmClient, Request};
use agen::tool::{
    Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOutput,
    ToolResultDisposition,
};
use async_trait::async_trait;
use error_contract::{
    Budget, CorrelationRef, DecodeFailure, FailureFacts, OperationContext, project_failure,
};
use futures::Stream;
use protocol::control_failure::RuntimeControlCause;
use protocol::{
    WorkerCommandAcknowledgement, WorkerCommandDisposition, WorkerCommandKind, WorkerStateSnapshot,
};
use serde_json::{Value, json};
use session_store::{
    Item, LogEntry, LoggedSessionHistoryOrigin, RestoredState, Store, WorkerSessionStore,
};
use tools::failure::{ToolFailureBoundary, execution_failure};
use worker::{
    SessionHistoryMetadata, Worker, WorkerFilesystemAuthority, WorkerManifest,
    WorkerWorkspaceContext,
};

const TOOL_NAME: &str = "ControlFailureFixture";
const CALL_ID: &str = "control-call-1";
const OUTPUT_BUDGET: usize = 64 * 1024;
const PRIVATE_OPERATION_REF: &str = "private-operation-canary";
const PRIVATE_CORRELATION: &str = "private-correlation-canary";

#[derive(Clone)]
struct FailureCase {
    cause: RuntimeControlCause,
    boundary: ToolFailureBoundary,
    result: Option<WorkerCommandAcknowledgement>,
    output_budget: usize,
}

impl FailureCase {
    fn rejected() -> Self {
        Self {
            cause: RuntimeControlCause::ExecutionRejected,
            boundary: ToolFailureBoundary::Execution,
            result: Some(WorkerCommandAcknowledgement {
                command_id: 7,
                command: WorkerCommandKind::Resume,
                disposition: WorkerCommandDisposition::InvalidState,
                state: WorkerStateSnapshot {
                    last_command_id: 7,
                    ..WorkerStateSnapshot::initial()
                },
            }),
            output_budget: OUTPUT_BUDGET,
        }
    }
}

struct ControlFailureTool {
    case: FailureCase,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for ControlFailureTool {
    async fn execute(
        &self,
        _input: &str,
        context: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        assert_eq!(context.call_id, CALL_ID);
        self.calls.fetch_add(1, Ordering::SeqCst);

        let mut facts = FailureFacts::new(self.case.cause).unwrap();
        let mut operation = OperationContext::new("worker_command", "admission").unwrap();
        operation.operation_ref = Some(PRIVATE_OPERATION_REF.into());
        facts.with_context(operation).unwrap();
        facts
            .set_correlation(CorrelationRef::new(PRIVATE_CORRELATION).unwrap())
            .unwrap();
        // RuntimeControlCause's domain projector deliberately discloses neither
        // reference, even though the common facts can retain them internally.
        let projected = project_failure::<error_contract::Tool, _>(&facts, &()).unwrap();
        Err(execution_failure(
            &projected,
            self.case.boundary,
            &self.case.result,
            self.case.output_budget,
        ))
    }
}

fn failure_tool(case: FailureCase, calls: Arc<AtomicUsize>) -> ToolDefinition {
    Arc::new(move || {
        (
            ToolMeta::new(TOOL_NAME)
                .description("Deterministic control operation failure")
                .input_schema(json!({"type": "object", "properties": {}})),
            Arc::new(ControlFailureTool {
                case: case.clone(),
                calls: calls.clone(),
            }) as Arc<dyn Tool>,
        )
    })
}

#[derive(Clone)]
struct ScriptedModel {
    root: PathBuf,
    requests: Arc<Mutex<Vec<Request>>>,
    committed_at_followup: Arc<Mutex<Option<Vec<LogEntry>>>>,
}

#[async_trait]
impl LlmClient for ScriptedModel {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }

    async fn stream(
        &self,
        request: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
    {
        let index = {
            let mut requests = self.requests.lock().unwrap();
            let index = requests.len();
            requests.push(request.clone());
            index
        };
        let mut events = match index {
            0 => vec![
                LlmEvent::tool_use_start(0, CALL_ID, TOOL_NAME),
                LlmEvent::tool_input_delta(0, "{}"),
                LlmEvent::tool_use_stop(0),
            ],
            1 => {
                // Observe the real file before accepting the follow-up request:
                // persisting only at run end would fail this assertion.
                let store = WorkerSessionStore::new(&self.root).unwrap();
                let session_id = store.session_id().unwrap().expect("session manifest");
                let segments = store.list_segments(session_id).unwrap();
                assert_eq!(segments.len(), 1, "fixture must not fork or compact");
                let entries = store.read_all(session_id, segments[0]).unwrap();
                let state = session_store::collect_state(&entries);
                assert_eq!(
                    single_result(&state.history),
                    single_result(&request.items),
                    "the model must receive the already-committed ToolResult"
                );
                *self.committed_at_followup.lock().unwrap() = Some(entries);
                vec![
                    LlmEvent::text_block_start(0),
                    LlmEvent::text_delta(0, "The command did not succeed."),
                    LlmEvent::text_block_stop(0, None),
                ]
            }
            _ => return Err(ClientError::Config("scripted model exhausted".into())),
        };
        events.push(LlmEvent::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }));
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}

fn single_result(items: &[Item]) -> &Item {
    let mut results = items
        .iter()
        .filter(|item| matches!(item, Item::ToolResult { call_id, .. } if call_id == CALL_ID));
    let result = results.next().expect("committed failure result missing");
    assert!(
        results.next().is_none(),
        "ToolResult must not be duplicated"
    );
    result
}

fn assert_private_context_absent(text: &str) {
    for private in [PRIVATE_OPERATION_REF, PRIVATE_CORRELATION] {
        assert!(
            !text.contains(private),
            "private context crossed the boundary"
        );
    }
}

fn provider_body(request: &Request) -> Value {
    let scheme = AnthropicScheme::new();
    scheme.build_request_body("fixture-model", request, &scheme.default_capability())
}

fn provider_result(body: &Value) -> &Value {
    let mut results = body["messages"]
        .as_array()
        .expect("Anthropic messages")
        .iter()
        .filter_map(|message| message["content"].as_array())
        .flatten()
        .filter(|part| part["type"] == "tool_result" && part["tool_use_id"] == CALL_ID);
    let result = results.next().expect("serialized ToolResult missing");
    assert!(results.next().is_none(), "serialized result was duplicated");
    result
}

struct SavedFailure {
    state: RestoredState,
    live_body: Value,
    replay_body: Value,
}

async fn run_and_reopen(case: FailureCase) -> SavedFailure {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("session");
    let model = ScriptedModel {
        root: root.clone(),
        requests: Arc::default(),
        committed_at_followup: Arc::default(),
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let manifest = WorkerManifest::from_toml(
        r#"
[worker]
name = "failure-history-fixture"
[model]
scheme = "anthropic"
model_id = "fixture-model"
[engine]
max_tokens = 128
[scope]
allow = []
"#,
    )
    .unwrap();
    let store = WorkerSessionStore::new(&root).unwrap();
    let mut engine =
        Engine::<_, agen::state::Mutable, SessionHistoryMetadata>::new_annotated(model.clone());
    engine.register_tool(failure_tool(case.clone(), calls.clone()));
    let mut worker = Worker::new(
        manifest,
        engine,
        store,
        WorkerWorkspaceContext::no_workspace(),
        WorkerFilesystemAuthority::None,
        worker::Scope::empty(),
    )
    .await
    .unwrap();
    // Use the same public persistence binding as Controller-created Workers.
    let writer = worker.log_writer_handle();
    worker.attach_log_writer(Arc::new(writer));
    worker.wire_history_persistence();
    let session_id = worker.session_id();
    let segment_id = worker.segment_id();
    tokio::time::timeout(
        Duration::from_secs(30),
        worker.run_text("Try the fixture control command once."),
    )
    .await
    .expect("Worker failure-result run timed out")
    .expect("a Tool execution failure must not fail the Worker run");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "no inferred retry grant");
    let live_history = worker.history();
    let live_entries = worker.sink().subscribe_with_snapshot().0;
    let requests = model.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2, "one Tool call and one follow-up reply");
    let prefix = model
        .committed_at_followup
        .lock()
        .unwrap()
        .clone()
        .expect("follow-up must observe a durable prefix");
    drop(worker);
    drop(model);

    // There is no surviving writer or store handle here. Open the actual Session
    // manifest and JSONL again, then use the production Store replay path.
    let reopened = WorkerSessionStore::new(&root).unwrap();
    assert_eq!(reopened.session_id().unwrap(), Some(session_id));
    let entries = reopened.read_all(session_id, segment_id).unwrap();
    assert_eq!(
        serde_json::to_value(&entries).unwrap(),
        serde_json::to_value(&live_entries).unwrap(),
        "the live mirror must agree with the reopened store"
    );
    assert_eq!(
        serde_json::to_value(&entries[..prefix.len()]).unwrap(),
        serde_json::to_value(&prefix).unwrap(),
        "run completion must not rewrite the prefix sent to the model"
    );
    let committed_result = entries
        .iter()
        .filter_map(|entry| match entry {
            LogEntry::AnnotatedToolResult { entry, .. } => Some(entry),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(committed_result.len(), 1);
    assert!(matches!(
        &committed_result[0].metadata.origin,
        LoggedSessionHistoryOrigin::ToolOutput { .. }
    ));
    let state = session_store::restore(&reopened, session_id, segment_id).unwrap();
    assert_eq!(
        session_store::to_logged(&state.history),
        session_store::to_logged(&live_history),
        "replay preserves the durable history projection, not ephemeral provider IDs"
    );
    assert!(!state.last_run_interrupted);
    assert_eq!(state.active_run_turn_count, None);
    assert_eq!(
        single_result(&state.history),
        single_result(&requests[1].items)
    );
    let raw =
        std::fs::read_to_string(root.join("segments").join(format!("{segment_id}.jsonl"))).unwrap();
    assert_private_context_absent(&raw);

    let live_body = provider_body(&requests[1]);
    let replay_request = Request {
        system_prompt: state.system_prompt.clone(),
        items: state.history.clone(),
        tools: requests[1].tools.clone(),
        config: state.config.clone(),
        ..Request::default()
    };
    let replay_body = provider_body(&replay_request);
    // Compare the failure block, not the whole body: replay also contains the
    // final assistant reply that was not yet present in the follow-up request.
    assert_eq!(provider_result(&live_body), provider_result(&replay_body));
    for body in [&live_body, &replay_body] {
        let bytes = serde_json::to_vec(body).unwrap();
        assert_private_context_absent(std::str::from_utf8(&bytes).unwrap());
        let result = provider_result(body);
        assert_eq!(result["is_error"], true);
        assert!(
            result["content"]
                .as_str()
                .unwrap()
                .contains(case.cause.explanation())
        );
    }
    SavedFailure {
        state,
        live_body,
        replay_body,
    }
}

fn assert_saved_report(saved: &SavedFailure, case: &FailureCase) -> Value {
    let Item::ToolResult {
        summary,
        content,
        disposition,
        is_error,
        attachments,
        ..
    } = single_result(&saved.state.history)
    else {
        unreachable!()
    };
    assert_eq!(*disposition, ToolResultDisposition::Error);
    assert!(*is_error);
    assert!(attachments.is_empty());
    assert_eq!(summary.as_str(), case.cause.explanation());
    let content = content
        .as_ref()
        .expect("structured error details persisted");
    let report: Value = serde_json::from_str(content).expect("complete bounded report JSON");
    assert_eq!(
        report["boundary"],
        serde_json::to_value(case.boundary).unwrap()
    );
    assert_eq!(
        report["result"],
        serde_json::to_value(&case.result).unwrap()
    );
    let cause_bytes = serde_json::to_vec(&report["failure"]).unwrap();
    let DecodeFailure::Known(received) = error_contract::decode_failure::<RuntimeControlCause>(
        &cause_bytes,
        Budget::new(OUTPUT_BUDGET, OUTPUT_BUDGET),
    ) else {
        panic!("persisted Runtime-control cause must remain a known failure")
    };
    assert_eq!(*received.facts().cause(), case.cause);
    assert_eq!(report["failure"]["explanation"], case.cause.explanation());
    assert!(received.facts().context().operations.is_empty());
    assert!(received.facts().context().correlation.is_none());
    for body in [&saved.live_body, &saved.replay_body] {
        assert!(
            provider_result(body)["content"]
                .as_str()
                .unwrap()
                .contains(content.as_str())
        );
    }
    report
}

#[tokio::test]
async fn rejected_command_keeps_cause_and_public_result_through_worker_store_and_model_request() {
    let case = FailureCase::rejected();
    let saved = run_and_reopen(case.clone()).await;
    let report = assert_saved_report(&saved, &case);
    let acknowledgement: WorkerCommandAcknowledgement =
        serde_json::from_value(report["result"].clone()).unwrap();
    assert_eq!(acknowledgement.command_id, 7);
    assert_eq!(
        acknowledgement.disposition,
        WorkerCommandDisposition::InvalidState
    );
    assert_eq!(report["failure"]["reason"], "execution_rejected");
}

#[tokio::test]
async fn transport_failure_keeps_observation_boundary_without_inventing_command_settlement() {
    let case = FailureCase {
        cause: RuntimeControlCause::TransportFailed,
        boundary: ToolFailureBoundary::Observation,
        result: None,
        output_budget: OUTPUT_BUDGET,
    };
    let saved = run_and_reopen(case.clone()).await;
    let report = assert_saved_report(&saved, &case);
    assert_eq!(report["boundary"], "observation");
    assert_eq!(report["failure"]["category"], "unknown");
    assert!(
        report["result"].is_null(),
        "no remote acknowledgement was observed"
    );
}

#[tokio::test]
async fn report_budget_failure_replays_as_error_with_cause_summary_and_no_partial_json() {
    let mut case = FailureCase::rejected();
    case.output_budget = 1;
    let saved = run_and_reopen(case.clone()).await;
    let Item::ToolResult {
        summary,
        content,
        disposition,
        is_error,
        ..
    } = single_result(&saved.state.history)
    else {
        unreachable!()
    };
    assert_eq!(*disposition, ToolResultDisposition::Error);
    assert!(*is_error);
    assert!(summary.starts_with(case.cause.explanation()));
    assert!(summary.contains("Failure details unavailable."));
    assert!(content.is_none(), "never persist truncated report JSON");
    assert_eq!(
        provider_result(&saved.replay_body)["content"],
        summary.as_str()
    );
}
