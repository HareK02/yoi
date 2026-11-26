//! Real Worker Flow installation -> compaction activation -> FsStore capture ->
//! SessionExplore tools -> durable transition. Only the LLM is scripted; this
//! checks evidence transport, not a real model's source/requirements judgment.

use super::*;
use agen::llm_client::event::{Event, ResponseStatus, StatusEvent, StopReason};
use agen::llm_client::types::Request;
use agen::llm_client::{ClientError, ResponseStream};
use agen::tool::ToolExecutionContext;
use futures::stream;
use serde_json::{Value, json};
use session_store::{FsStore, LoggedHistoryEntry};

#[derive(Clone, Default)]
struct CaptureClient {
    steps: Arc<Mutex<VecDeque<(String, Value)>>>,
    responses: Arc<Mutex<Vec<Item>>>,
    append_during_verification: Arc<Mutex<Option<(FsStore, SessionId, SegmentId)>>>,
    verifier_calls: Arc<AtomicUsize>,
    boundaries: Arc<Mutex<Vec<Value>>>,
}

fn tool_events(name: &str, input: Value) -> Vec<Event> {
    vec![
        Event::tool_use_start(0, name, name),
        Event::tool_input_delta(0, input.to_string()),
        Event::tool_use_stop(0),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]
}

#[async_trait::async_trait]
impl LlmClient for CaptureClient {
    async fn stream(&self, request: Request) -> Result<ResponseStream, ClientError> {
        let verifier = request
            .tools
            .iter()
            .any(|tool| tool.name == "FinishFlowVerification");
        let compactor = request
            .tools
            .iter()
            .any(|tool| tool.name == "write_summary");
        let events = if verifier {
            self.verifier_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(boundary) = request.items.iter().filter_map(Item::as_text).find_map(|text| serde_json::from_str::<Value>(text).ok().and_then(|input| input.get("parent_capture").cloned())) {
                self.boundaries.lock().unwrap().push(boundary);
            }
            self.responses.lock().unwrap().extend(request.items.iter().filter(|item| item.is_tool_result()).cloned());
            // Appending after the first exploration result must not change this
            // verifier's already selected capture.
            if request.items.iter().any(Item::is_tool_result) {
                if let Some((store, session, segment)) = self.append_during_verification.lock().unwrap().take() {
                    store.append(session, segment, &input_record("LATE_COMMITTED_PROOF")).unwrap();
                }
            }
            match self.steps.lock().unwrap().pop_front() {
                Some((name, mut input)) => {
                    if name == "FinishFlowVerification" {
                        let reads = outputs(self, "ReadEntry").join("\n");
                        if !reads.contains("CURRENT_SOURCE_PROOF source=current requirements=r1 validated=true published=true") || !reads.contains("OLD_SOURCE_PROOF") {
                            input["results"][0]["verdict"] = json!("indeterminate");
                            input["results"][0]["rationale"] = json!("Exact current/adopted proof is unavailable in the captured reads.");
                        }
                    }
                    tool_events(&name, input)
                },
                None => text_events(),
            }
        } else if compactor && !request.items.iter().any(|item| matches!(item, Item::ToolResult { call_id, .. } if call_id.as_str() == "write_summary")) {
            tool_events("write_summary", json!({"text":"## Goal\nPreserve evidence.\n## Decisions\nNone.\n## Progress\nReady.\n## Next\nVerify.\n## References\nNone."}))
        } else {
            text_events()
        };
        Ok(Box::pin(stream::iter(events.into_iter().map(Ok))))
    }

    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
}

fn text_events() -> Vec<Event> {
    vec![
        Event::text_block_start(0),
        Event::text_delta(0, "recorded"),
        Event::text_block_stop(0, Some(StopReason::EndTurn)),
        Event::Status(StatusEvent {
            status: ResponseStatus::Completed,
        }),
    ]
}

fn input_record(text: &str) -> LogEntry {
    LogEntry::AnnotatedUserInput {
        ts: 1,
        history: vec![crate::session_history::test_logged_history_entry(
            Item::user_message(text),
        )],
        segments: vec![Segment::text(text)],
        extensions: vec![],
    }
}

fn flow_state() -> flow::FlowRuntimeState {
    let definition = flow::compile_flow_source(r#"{
        schema_version = 1;
        name = "capture-regression";
        initial = "implement";
        states = {
            implement = { instructions = "Implement."; transitions.review = { target = "review"; condition = "Read the current committed proof and adopted old proof."; }; };
            review = { instructions = "Review."; terminal = true; };
        };
    }"#).unwrap();
    flow::FlowRuntimeState::start(
        &flow::ResolvedFlowSource {
            selector: "workspace:capture-regression".parse().unwrap(),
            workspace_id: "fixture".into(),
            flow_id: "fixture".into(),
            content_digest: definition.content_digest.clone(),
            definition,
        },
        "capture-instance",
    )
    .unwrap()
    .0
}

async fn worker(store: FsStore, client: CaptureClient) -> Worker<CaptureClient, FsStore> {
    let manifest = WorkerManifest::from_toml(
        r#"
[worker]
name = "flow-capture-regression"
[model]
scheme = "anthropic"
model_id = "scripted"
[engine]
[scope]
allow = []
"#,
    )
    .unwrap();
    let mut worker = Worker::new(
        manifest,
        Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(client),
        store,
        WorkerWorkspaceContext::no_workspace(),
        WorkerFilesystemAuthority::None,
        Scope::empty(),
    )
    .await
    .unwrap();
    worker.ensure_segment_head().await.unwrap();
    *worker.flow_runtime_state.lock().unwrap() = Some(flow_state());
    let report = worker.install_runtime_flow_transition_feature().unwrap();
    assert!(!report.has_errors());
    worker
}

fn search(query: &str) -> (String, Value) {
    ("SearchEntries".into(), json!({"query":query,"limit":100}))
}
fn read(reference: &str) -> (String, Value) {
    (
        "ReadEntry".into(),
        json!({"entry_ref":reference,"mode":"full"}),
    )
}
fn finish() -> (String, Value) {
    (
        "FinishFlowVerification".into(),
        json!({"results":[
            {"transition_id":"review","verdict":"met","rationale":"Scripted evidence transport check; not a real model judgment."},
            {"transition_id":"$cancel","verdict":"not_met","rationale":"No exceptional condition."}
        ]}),
    )
}

async fn transition(worker: &Worker<CaptureClient, FsStore>) -> Value {
    let server = worker.engine.as_ref().unwrap().tool_server_handle();
    server.flush_pending();
    let tool = server.get_tool("RequestFlowTransition").unwrap().1;
    let output = tool
        .execute(
            r#"{"reason":"check committed capture"}"#,
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
    serde_json::from_str(output.content.as_deref().unwrap()).unwrap()
}

fn outputs(client: &CaptureClient, call: &str) -> Vec<String> {
    client
        .responses
        .lock()
        .unwrap()
        .iter()
        .filter_map(|item| match item {
            Item::ToolResult {
                call_id,
                summary,
                content,
                ..
            } if call_id.as_str() == call => Some(format!(
                "{summary}\n{}",
                content.as_deref().unwrap_or_default()
            )),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn installed_flow_follows_compaction_and_freezes_adopted_committed_evidence() {
    let root = tempfile::tempdir().unwrap();
    let store = FsStore::new(root.path()).unwrap();
    let client = CaptureClient::default();
    let mut worker = worker(store.clone(), client.clone()).await;
    worker
        .run_text("OLD_SOURCE_PROOF source=old requirements=r1")
        .await
        .unwrap();
    let old = worker.self_session_reader().capture().unwrap();
    let old_ref = old.search(&crate::session_capture::SearchOptions {
        query: "OLD_SOURCE_PROOF".into(),
        ..Default::default()
    })[0]
        .id
        .to_string();
    for index in 0..40 {
        worker
            .run_text(format!("continuation evidence {index}"))
            .await
            .unwrap();
    }
    let original = worker.segment_id();
    let compacted = worker.compact(0).await.unwrap();
    assert_ne!(original, compacted);
    // A late parent suffix, foreign Session and in-memory-only item are not
    // authorized committed evidence for the active lineage.
    let excluded = input_record("UNADOPTED_SUFFIX");
    let excluded_ref = record_ref(&excluded);
    store
        .append(worker.session_id(), original, &excluded)
        .unwrap();
    let foreign_session = SessionId::now_v7();
    let foreign_segment = SegmentId::now_v7();
    let foreign = crate::session_history::test_logged_history_entry(Item::user_message(
        "FOREIGN_SESSION_PROOF",
    ));
    let foreign_ref =
        crate::session_capture::SessionEntryRef::from_history_entry_id(&foreign.metadata.entry_id)
            .to_string();
    store
        .create_segment(
            foreign_session,
            foreign_segment,
            &[seed(foreign_session, vec![foreign])],
        )
        .unwrap();
    worker
        .run_text(
            "CURRENT_SOURCE_PROOF source=current requirements=r1 validated=true published=true",
        )
        .await
        .unwrap();
    let current = worker.self_session_reader().capture().unwrap();
    let current_ref = current.search(&crate::session_capture::SearchOptions {
        query: "CURRENT_SOURCE_PROOF".into(),
        ..Default::default()
    })[0]
        .id
        .to_string();
    let uncommitted = history_entry(
        Item::user_message("UNCOMMITTED_PROOF"),
        WorkerHistoryProvenance::LocalHumanInput,
    );
    let uncommitted_ref = crate::session_capture::SessionEntryRef::from_history_entry_id(
        &uncommitted.annotation.entry_id,
    )
    .to_string();
    worker.session.history_mut().push_entry(uncommitted);
    *client.append_during_verification.lock().unwrap() =
        Some((store.clone(), worker.session_id(), compacted));
    *client.steps.lock().unwrap() = VecDeque::from([
        search("CURRENT_SOURCE_PROOF"),
        read(&current_ref),
        read(&old_ref),
        search("LATE_COMMITTED_PROOF"),
        search("UNADOPTED_SUFFIX"),
        search("FOREIGN_SESSION_PROOF"),
        search("UNCOMMITTED_PROOF"),
        read(&excluded_ref),
        read(&foreign_ref),
        read(&uncommitted_ref),
        finish(),
    ]);
    let result = transition(&worker).await;
    let reads = outputs(&client, "ReadEntry").join("\n");
    assert!(
        reads.contains(
            "CURRENT_SOURCE_PROOF source=current requirements=r1 validated=true published=true"
        ),
        "{reads}"
    );
    assert!(reads.contains("OLD_SOURCE_PROOF"), "{reads}");
    for excluded in [
        "UNADOPTED_SUFFIX",
        "FOREIGN_SESSION_PROOF",
        "UNCOMMITTED_PROOF",
        "LATE_COMMITTED_PROOF",
    ] {
        assert!(!reads.contains(excluded), "{excluded}: {reads}");
    }
    let searches = outputs(&client, "SearchEntries").join("\n");
    assert!(searches.contains("CURRENT_SOURCE_PROOF"), "{searches}");
    for excluded in [
        "UNADOPTED_SUFFIX",
        "FOREIGN_SESSION_PROOF",
        "UNCOMMITTED_PROOF",
        "LATE_COMMITTED_PROOF",
    ] {
        assert!(!searches.contains(excluded), "{excluded}: {searches}");
    }
    assert_eq!(result["entered_state"], "review");
    let boundaries = client.boundaries.lock().unwrap();
    assert!(!boundaries.is_empty());
    for boundary in boundaries.iter() {
        assert_eq!(boundary["session_id"], worker.session_id().to_string());
        assert_eq!(boundary["active_segment_id"], compacted.to_string());
        assert_eq!(boundary["capture_id"], current.capture_id());
    }
    drop(boundaries);
    let fresh = worker.self_session_reader().capture().unwrap();
    assert_ne!(fresh.capture_id(), current.capture_id());
    assert_eq!(
        fresh
            .search(&crate::session_capture::SearchOptions {
                query: "LATE_COMMITTED_PROOF".into(),
                ..Default::default()
            })
            .len(),
        1
    );
    let saved = store.read_all(worker.session_id(), compacted).unwrap();
    let restored = session_store::collect_state(&saved);
    let state = restored_flow_runtime_state(&restored.extensions)
        .unwrap()
        .unwrap();
    assert_eq!(state.instance.current_state.as_str(), "review");
}

fn record_ref(record: &LogEntry) -> String {
    let LogEntry::AnnotatedUserInput { history, .. } = record else {
        unreachable!()
    };
    crate::session_capture::SessionEntryRef::from_history_entry_id(&history[0].metadata.entry_id)
        .to_string()
}

fn seed(session_id: SessionId, history: Vec<LoggedHistoryEntry>) -> LogEntry {
    LogEntry::AnnotatedSegmentStart {
        ts: 0,
        session_id,
        history,
        system_prompt: None,
        config: Default::default(),
        compacted_from: None,
        forked_from: None,
    }
}

#[tokio::test]
async fn missing_adoption_boundary_fails_before_supplying_a_verifier() {
    let root = tempfile::tempdir().unwrap();
    let store = FsStore::new(root.path()).unwrap();
    let client = CaptureClient::default();
    let worker = worker(store.clone(), client.clone()).await;
    let original = worker.segment_id();
    let broken = SegmentId::now_v7();
    let mut header = seed(worker.session_id(), vec![]);
    let LogEntry::AnnotatedSegmentStart { compacted_from, .. } = &mut header else {
        unreachable!()
    };
    *compacted_from = Some(session_store::SegmentOrigin {
        segment_id: original,
        at_turn_index: 0,
    });
    store
        .create_segment(worker.session_id(), broken, &[header])
        .unwrap();
    // Corrupt-storage fixture: unlike the success case, deliberately omit the
    // compaction opening summary's adopted identities. Never fall back to the
    // installation segment or run the model against partial evidence.
    worker.segment_state.set_location(SegmentLocation {
        session_id: worker.session_id(),
        segment_id: broken,
    });
    let result = transition(&worker).await;
    assert!(result["entered_state"].is_null());
    assert_eq!(result["rejection"]["code"], "verifier_failed");
    let message = result["rejection"]["message"].as_str().unwrap();
    assert!(
        message.contains("capture committed Flow parent Session"),
        "{message}"
    );
    assert!(
        message.contains("compaction source is unavailable"),
        "{message}"
    );
    assert_eq!(client.verifier_calls.load(Ordering::SeqCst), 0);
    let saved = store.read_all(worker.session_id(), broken).unwrap();
    let restored = session_store::collect_state(&saved);
    assert_eq!(
        restored_flow_runtime_state(&restored.extensions)
            .unwrap()
            .unwrap()
            .instance
            .current_state
            .as_str(),
        "implement"
    );
}
