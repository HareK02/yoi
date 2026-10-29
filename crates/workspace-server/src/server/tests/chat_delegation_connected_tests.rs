//! Async-chat consumer integration (`cargo test -p yoi-workspace-server --lib
//! chat_delegation_connected`). Public chat/Worker tools, signed Backend/Runtime,
//! Controller, grants, SQLite, Session and Workdir effects are real. Only the
//! external model is scripted/gated; the shared RAII harness owns all resources.
use super::*;
use agen::llm_client::{
    event::{Event, StopReason},
    types::Item,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use worker_runtime::worker_backend::RuntimeWorkerFactory;
use worker_runtime::working_directory::WorkingDirectoryMaterializer;

const CHILD: &str = "BOUNDED_CHAT_BACKGROUND_WORK";
const REPORT: &str = "BACKGROUND_EVIDENCE_FOR_ORIGINAL_REQUEST";
const PUBLIC_A: &str = "Evaluated background evidence for A";
const PUBLIC_B: &str = "Independent answer for B";

fn operation(name: &str, collection: bool) -> ScopedPermission {
    ScopedPermission {
        operation: name.into(),
        resource: if collection {
            PermissionResource::Workspace {
                workspace_id: TEST_WORKSPACE_ID.into(),
            }
        } else {
            PermissionResource::WorkerOperation {
                workspace_id: TEST_WORKSPACE_ID.into(),
            }
        },
    }
}
fn output(request: &agen::llm_client::Request, id: &str) -> Option<(bool, String)> {
    request.items.iter().find_map(|item| match item {
        Item::ToolResult {
            call_id,
            is_error,
            content,
            summary,
            ..
        } if call_id == id => Some((
            *is_error,
            content.clone().unwrap_or_else(|| summary.clone()),
        )),
        _ => None,
    })
}
fn value(request: &agen::llm_client::Request, id: &str) -> Value {
    let (error, text) = output(request, id).unwrap_or_else(|| panic!("missing tool result {id}"));
    assert!(!error, "{id}: {text}");
    serde_json::from_str(&text).unwrap_or_else(|_| panic!("{id}: {text}"))
}
fn call(id: &str, tool: &str, input: Value) -> Vec<Event> {
    vec![
        Event::tool_use_start(0, id, tool),
        Event::tool_input_delta(0, input.to_string()),
        Event::tool_use_stop(0),
    ]
}
fn done() -> Vec<Event> {
    vec![
        Event::text_block_start(0),
        Event::text_delta(0, "PRIVATE_CHAT_AND_WORKER_OUTPUT"),
        Event::text_block_stop(0, Some(StopReason::EndTurn)),
    ]
}

#[derive(Default)]
struct Context {
    a: String,
    b: String,
    workdir: Option<String>,
    permissions: Vec<ScopedPermission>,
    spawned: Option<RuntimeWorkerRef>,
    ticket: Option<String>,
    denials: Vec<(String, Value, String)>,
}
#[derive(Clone)]
struct DelegationModel {
    context: Arc<Mutex<Context>>,
    body_phase: Arc<AtomicUsize>,
    child_phase: Arc<AtomicUsize>,
    child_started: tokio::sync::mpsc::Sender<()>,
    release_work: Arc<tokio::sync::Semaphore>,
    completed: tokio::sync::mpsc::Sender<()>,
    private_report: tokio::sync::mpsc::Sender<()>,
    release_send: Arc<tokio::sync::Semaphore>,
    lose_spawn_ack: bool,
}
impl DelegationModel {
    fn spawn(&self, page: &Value, request: &agen::llm_client::Request) -> Value {
        let c = self.context.lock().unwrap();
        let message = page["conversation"]["last_user_message_id"]
            .as_str()
            .unwrap();
        let mut input = json!({"runtime_id":CONNECTED_RUNTIME,"profile":"project:chat-background",
            "operation_id":format!("chat:{}:{message}", c.a),
            "display_name":"Bounded background chat work",
            "initial_submit":[{"kind":"text","content":format!("{CHILD}; conversation={}; message={message}", c.a)}],
            "execution_grant":{"permissions":c.permissions,"redelegation":[]}});
        if let Some(id) = &c.workdir {
            input["working_directory_id"] = json!(id);
        }
        if let Some(ticket) = &c.ticket {
            let current = value(request, "ticket-current");
            let assignment = current["assignment_tracking"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["role"] == "orchestrator")
                .expect("current Subject responsibility");
            assert_eq!(assignment["principal"]["kind"], "subject");
            input["ticket_id"] = json!(ticket);
            input["orchestrator_assignment_id"] = assignment["assignment_id"].clone();
        }
        input
    }
    fn send(
        &self,
        request: &agen::llm_client::Request,
        read: &str,
        text: &str,
        operation: &str,
        work: bool,
    ) -> Value {
        let page = value(request, read);
        let message = &page["conversation"]["last_user_message_id"];
        let mut input = json!({"conversation_id":page["conversation"]["id"],"operation_id":operation,
            "text":text,"reply_to":message,"expected_last_user_message_id":message});
        if work {
            let child = value(request, "spawn-retry");
            input["work_ref"] = json!(format!(
                "worker:{}/{}",
                child["runtime_id"].as_str().unwrap(),
                child["worker_id"].as_str().unwrap()
            ));
        }
        input
    }
}
#[async_trait::async_trait]
impl agen::llm_client::LlmClient for DelegationModel {
    fn clone_boxed(&self) -> Box<dyn agen::llm_client::LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        request: agen::llm_client::Request,
    ) -> std::result::Result<agen::llm_client::client::ResponseStream, agen::llm_client::ClientError>
    {
        let child = request.items.iter().any(|item| matches!(item, Item::Message { content, .. } if serde_json::to_string(content).unwrap().contains(CHILD)));
        let ticket = self.context.lock().unwrap().ticket.clone();
        let messages = request
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Message { content, .. } => Some(serde_json::to_string(content).unwrap()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let preliminary = if child && self.child_phase.load(Ordering::SeqCst) >= 5 {
            // Assignment-change notifications can reach the finished background
            // Worker; they do not reissue the original chat work.
            Some(done())
        } else if child
            && ticket.is_some()
            && self.child_phase.load(Ordering::SeqCst) == 3
            && output(&request, "ticket-result").is_none()
        {
            Some(call(
                "ticket-result",
                "TicketImplementationResult",
                json!({"ticket":ticket,"kind":"result","body":"Verified real README Read/Write for the chat request"}),
            ))
        } else if !child {
            let phase = self.body_phase.load(Ordering::SeqCst);
            if phase >= 13
                || (phase == 0
                    && !messages.contains("Please perform bounded background work for A"))
                || (phase == 5 && !messages.contains("A separate question B"))
                || (phase == 8 && !messages.contains(REPORT))
                || (phase == 11 && !messages.contains("Attempt follow-up"))
            {
                // Business notifications are private context, not a new chat request
                // or permission to advance the scripted consumer's decision.
                Some(done())
            } else if ticket.is_some() && phase == 0 && output(&request, "ticket-current").is_none()
            {
                Some(call("ticket-current", "ShowTicket", json!({"id":ticket})))
            } else if ticket.is_some() && phase == 1 {
                let denials = self.context.lock().unwrap().denials.clone();
                let mut next = None;
                for (id, overrides, reason) in denials {
                    if let Some((error, text)) = output(&request, &id) {
                        assert!(
                            error && text.contains(&reason),
                            "public chat {id} must exercise {reason}, not another fence: {text}"
                        );
                    } else {
                        let mut input = self.spawn(&value(&request, "origin-read"), &request);
                        for (name, value) in overrides.as_object().unwrap() {
                            input[name] = value.clone();
                        }
                        input["operation_id"] = json!(id);
                        next = Some(call(&id, "WorkerSpawn", input));
                        break;
                    }
                }
                next
            } else if ticket.is_some()
                && phase == 8
                && output(&request, "ticket-evaluate").is_none()
            {
                let report = request
                    .items
                    .iter()
                    .find_map(|item| {
                        let Item::Message { content, .. } = item else {
                            return None;
                        };
                        content.iter().find_map(|part| {
                            let agen::llm_client::types::ContentPart::Text { text } = part else {
                                return None;
                            };
                            text.find(REPORT).and_then(|offset| {
                                serde_json::Deserializer::from_str(&text[offset + REPORT.len()..])
                                    .into_iter::<Value>()
                                    .next()
                                    .transpose()
                                    .expect("report JSON")
                            })
                        })
                    })
                    .expect("ordinary Notify report reference");
                assert_eq!(report["ticket"], json!(ticket));
                Some(call(
                    "ticket-evaluate",
                    "TicketImplementationResultQuery",
                    json!({"ticket":ticket,"result_id":report["result_id"]}),
                ))
            } else if ticket.is_some() && phase == 10 {
                let current = value(&request, "ticket-publication-recheck");
                let initial = value(&request, "ticket-current");
                if current["assignment_tracking"][0]["assignment_id"]
                    != initial["assignment_tracking"][0]["assignment_id"]
                {
                    if let Some((error, text)) = output(&request, "old-report-spawn") {
                        assert!(
                            error && text.contains("assignment"),
                            "old report cannot authorize a new Ticket execution: {text}"
                        );
                        self.body_phase.fetch_add(1, Ordering::SeqCst);
                        self.completed.try_send(()).unwrap();
                        Some(done())
                    } else {
                        let mut input = self.spawn(&value(&request, "origin-read"), &request);
                        input["operation_id"] = json!("old-ticket-report-new-attempt");
                        Some(call("old-report-spawn", "WorkerSpawn", input))
                    }
                } else if output(&request, "a-send").is_none() {
                    Some(call(
                        "a-send",
                        "ConversationSend",
                        self.send(&request, "a-reread", PUBLIC_A, "answer-a", true),
                    ))
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };
        if let Some(events) = preliminary {
            return Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))));
        }
        let events = if child {
            let phase = self.child_phase.fetch_add(1, Ordering::SeqCst);
            let workdir = self.context.lock().unwrap().workdir.is_some();
            match phase {
                0 => {
                    self.child_started.try_send(()).unwrap();
                    tokio::time::timeout(
                        std::time::Duration::from_secs(60),
                        self.release_work.acquire(),
                    )
                    .await
                    .expect("B must be answered before releasing background work")
                    .unwrap()
                    .forget();
                    if workdir {
                        call("work-read", "Read", json!({"file_path":"README.md"}))
                    } else {
                        call("work-query", "QueryObjective", json!({"limit":1}))
                    }
                }
                1 if workdir => {
                    let (error, text) = output(&request, "work-read").unwrap();
                    assert!(
                        !error && text.contains("clean"),
                        "real background Read: {text}"
                    );
                    call(
                        "work-write",
                        "Write",
                        json!({"file_path":"README.md","content":"background changed after real Read\n"}),
                    )
                }
                1 | 2 if phase == if workdir { 2 } else { 1 } => {
                    let (error, text) =
                        output(&request, if workdir { "work-write" } else { "work-query" })
                            .unwrap();
                    assert!(!error, "bounded domain/file work: {text}");
                    call("reply-origin", "WorkerList", json!({}))
                }
                2 | 3 if phase == if workdir { 3 } else { 2 } => {
                    let list = value(&request, "reply-origin");
                    let origins = list["items"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|item| item["relation"] == "reply")
                        .collect::<Vec<_>>();
                    assert_eq!(origins.len(), 1, "confirmed origin: {list}");
                    assert_eq!(origins[0]["permissions"], json!(["notify"]));
                    let c = self.context.lock().unwrap();
                    let report = if c.ticket.is_some() {
                        let result = value(&request, "ticket-result");
                        format!(
                            "{REPORT}{}",
                            json!({"conversation":c.a,"ticket":c.ticket,"result_id":result["result_id"]})
                        )
                    } else {
                        format!(
                            "{REPORT}; conversation={}; work={}",
                            c.a,
                            if workdir {
                                "README read/write"
                            } else {
                                "bounded objective query"
                            }
                        )
                    };
                    call(
                        "report",
                        "WorkerNotify",
                        json!({"subject":origins[0]["subject"],"content":report}),
                    )
                }
                3 | 4 => {
                    assert_eq!(value(&request, "report")["state"], "accepted");
                    done()
                }
                _ => panic!("unexpected background model phase {phase}"),
            }
        } else {
            assert!(
                !request.tools.iter().any(|tool| matches!(
                    tool.name.as_str(),
                    "Bash" | "Read" | "Write" | "Edit" | "SubWorkerSpawn"
                )),
                "actual async-chat must stay restricted"
            );
            let phase = self.body_phase.fetch_add(1, Ordering::SeqCst);
            match phase {
                0 => call(
                    "origin-read",
                    "ConversationRead",
                    json!({"conversation_id":self.context.lock().unwrap().a}),
                ),
                1 => call(
                    "spawn-first",
                    "WorkerSpawn",
                    self.spawn(&value(&request, "origin-read"), &request),
                ),
                2 => {
                    let (error, text) = output(&request, "spawn-first").unwrap();
                    if self.lose_spawn_ack {
                        assert!(
                            error && text.contains("unknown"),
                            "lost ACK must not claim acceptance/rejection: {text}"
                        );
                    } else {
                        assert!(!error, "first spawn: {text}");
                    }
                    call(
                        "spawn-retry",
                        "WorkerSpawn",
                        self.spawn(&value(&request, "origin-read"), &request),
                    )
                }
                3 => {
                    let retry = value(&request, "spawn-retry");
                    if !self.lose_spawn_ack {
                        assert_eq!(
                            value(&request, "spawn-first")["worker_id"],
                            retry["worker_id"]
                        );
                    }
                    call(
                        "spawn-exact-replay",
                        "WorkerSpawn",
                        self.spawn(&value(&request, "origin-read"), &request),
                    )
                }
                4 => {
                    let child = value(&request, "spawn-retry");
                    assert_eq!(
                        value(&request, "spawn-exact-replay")["worker_id"],
                        child["worker_id"]
                    );
                    self.context.lock().unwrap().spawned = Some(RuntimeWorkerRef::new(
                        child["runtime_id"].as_str().unwrap(),
                        child["worker_id"].as_str().unwrap(),
                    ));
                    done()
                }
                5 => call(
                    "b-read",
                    "ConversationRead",
                    json!({"conversation_id":self.context.lock().unwrap().b}),
                ),
                6 => call(
                    "b-send",
                    "ConversationSend",
                    self.send(&request, "b-read", PUBLIC_B, "answer-b", false),
                ),
                7 => {
                    value(&request, "b-send");
                    done()
                }
                8 => {
                    assert!(request.items.iter().any(|item| matches!(item, Item::Message { content, .. } if serde_json::to_string(content).unwrap().contains(REPORT))), "ordinary report must reach model as private notification");
                    call(
                        "a-reread",
                        "ConversationRead",
                        json!({"conversation_id":self.context.lock().unwrap().a}),
                    )
                }
                9 => {
                    if let Some(ticket) = &ticket {
                        let result = value(&request, "ticket-evaluate");
                        assert_eq!(result["ticket_id"], json!(ticket));
                        assert_eq!(
                            result["request"]["body"],
                            "Verified real README Read/Write for the chat request"
                        );
                    }
                    self.private_report.try_send(()).unwrap();
                    tokio::time::timeout(
                        std::time::Duration::from_secs(60),
                        self.release_send.acquire(),
                    )
                    .await
                    .expect("inspect private report before explicit publication")
                    .unwrap()
                    .forget();
                    if ticket.is_some() {
                        call(
                            "ticket-publication-recheck",
                            "ShowTicket",
                            json!({"id":ticket}),
                        )
                    } else {
                        call(
                            "a-send",
                            "ConversationSend",
                            self.send(&request, "a-reread", PUBLIC_A, "answer-a", true),
                        )
                    }
                }
                10 => {
                    value(&request, "a-send");
                    self.completed.try_send(()).unwrap();
                    done()
                }
                11 => {
                    let child = value(&request, "spawn-retry");
                    call(
                        "revoked-notify",
                        "WorkerNotify",
                        json!({"subject":{"kind":"runtime_worker","runtime_id":child["runtime_id"],"worker_id":child["worker_id"]},"content":"MUST_NOT_REACH_REVOKED_CHILD"}),
                    )
                }
                12 => {
                    let (error, text) = output(&request, "revoked-notify").unwrap();
                    assert!(
                        error,
                        "revocation must be a public Tool refusal, never success: {text}"
                    );
                    self.completed.try_send(()).unwrap();
                    done()
                }
                _ => panic!("unexpected chat model phase {phase}"),
            }
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}

fn publish_profiles(api: &WorkspaceApi) {
    let state = api
        .config_store
        .load_workspace_config(TEST_WORKSPACE_ID)
        .unwrap()
        .unwrap();
    let main = state
        .snapshot
        .get(&config_source::VirtualPath::parse("main.dcdl").unwrap())
        .unwrap();
    let content = r#"{ profile = { default_profile = "project:async-chat"; entries = [
        { selector = "project:async-chat"; label = "Async chat"; description = "Unmodified built-in chat";
          profile = import "$builtin/profiles/async-chat.dcdl"; },
        { selector = "project:chat-background"; label = "Background work"; description = "Bounded file or domain work";
          profile = (import "$builtin/profiles/companion.dcdl") // { slug = "chat-background"; scope = "workspace_write";
            feature = { subjektiv = { enabled = false; }; worker = { enabled = true; direct_spawn = false; }; };
            permissions = { default_action = "deny"; rule = [
              { tool = "Read"; pattern = "*"; action = "allow"; },
              { tool = "Write"; pattern = "*"; action = "allow"; },
              { tool = "QueryObjective"; pattern = "*"; action = "allow"; },
              { tool = "WorkerList"; pattern = "*"; action = "allow"; },
              { tool = "WorkerNotify"; pattern = "*"; action = "allow"; },
              { tool = "TicketImplementationResult"; pattern = "*"; action = "allow"; }
            ]; };
          }; }
    ]; }; }"#;
    commit_workspace_config_tree(
        api,
        TEST_WORKSPACE_ID,
        &ConfigCommitRequest {
            base_digest: state.snapshot.digest.clone(),
            entrypoints: state.contract.entrypoints.clone(),
            changes: vec![config_source::ConfigTreeChange::Update {
                path: main.path.clone(),
                expected_digest: main.content_digest.clone(),
                content: content.into(),
            }],
        },
    )
    .unwrap();
}
async fn account_request(
    h: &ConnectedWorkerHarness,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Value {
    let (status, result) = response(&h.api, human_request(&h.owner, method, path, body)).await;
    assert!(status.is_success(), "{method} {path}: {status}: {result}");
    result
}
fn messages(conversation: &str) -> String {
    format!("/api/w/{TEST_WORKSPACE_ID}/chat/conversations/{conversation}/messages")
}
async fn idle(
    events: &mut tokio::sync::broadcast::Receiver<
        worker_runtime::observation::WorkerObservationEvent,
    >,
    worker: &str,
    phase: &AtomicUsize,
    expected: usize,
) {
    tokio::time::timeout(std::time::Duration::from_secs(60),async {
        loop {
            let event = events.recv().await.unwrap();
            if phase.load(Ordering::SeqCst) >= expected && event.worker_ref.worker_id.to_string() == worker && matches!(event.payload,protocol::Event::WorkerState {snapshot} if snapshot.state == protocol::WorkerState::Idle) {break;}
        }
    }).await.expect("real chat Controller Idle boundary");
}
enum Scenario {
    Workdir,
    Workdirless,
    Ticket,
    ReassignedTicket,
}
async fn connected_chat_delegation(scenario: Scenario) {
    let explicit_workdir = !matches!(scenario, Scenario::Workdirless);
    let ticket_mode = matches!(scenario, Scenario::Ticket | Scenario::ReassignedTicket);
    let reassign_before_evaluation = matches!(scenario, Scenario::ReassignedTicket);
    let lose_spawn_ack = matches!(scenario, Scenario::Workdir | Scenario::Ticket);
    let context = Arc::new(Mutex::new(Context::default()));
    let (started, mut start) = tokio::sync::mpsc::channel(1);
    let (completed, mut completion) = tokio::sync::mpsc::channel(2);
    let (private_report, mut private_input) = tokio::sync::mpsc::channel(1);
    let release_send = Arc::new(tokio::sync::Semaphore::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let model = DelegationModel {
        context: context.clone(),
        body_phase: Arc::new(AtomicUsize::new(0)),
        child_phase: Arc::new(AtomicUsize::new(0)),
        child_started: started,
        release_work: release.clone(),
        private_report,
        release_send: release_send.clone(),
        completed,
        lose_spawn_ack,
    };
    let body_phase = model.body_phase.clone();
    let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let dropped_response = dropped.clone();
    let h = ConnectedWorkerHarness::start_with_layer(model,move |router| router.layer(axum::middleware::from_fn(
        move |request:axum::extract::Request,next:axum::middleware::Next| {
            let dropped = dropped_response.clone();
            async move {
                let spawn = request.method() == axum::http::Method::POST && request.uri().path() == format!("/api/w/{TEST_WORKSPACE_ID}/worker-control/workers");
                let response = next.run(request).await;
                if spawn && response.status().is_success() && lose_spawn_ack && !dropped.swap(true,Ordering::SeqCst) {
                    // Discard only the response after real registry/grant persistence.
                    (StatusCode::GATEWAY_TIMEOUT,Json(json!({"error":"Gateway Timeout","message":"spawn outcome unknown; retry original operation and input","diagnostics":[]}))).into_response()
                } else {response}
            }
        }
    ))).await;
    publish_profiles(&h.api);
    let mut child_permissions = vec![
        operation("worker.messaging", false),
        operation("worker.list", true),
    ];
    let mut readme = None;
    let selector = if ticket_mode {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(h._dir.path())
                .args(["branch", "chat-ticket"])
                .status()
                .unwrap()
                .success()
        );
        "chat-ticket"
    } else {
        "HEAD"
    };
    if explicit_workdir {
        let created = account_request(&h,"POST",&format!("/api/w/{TEST_WORKSPACE_ID}/working-directories"),Some(json!({"runtime_id":CONNECTED_RUNTIME,"repository_key":"test-repository","selector":selector,"operation_id":"chat-workdir"}))).await;
        let id = created["item"]["working_directory_id"]
            .as_str()
            .unwrap()
            .to_owned();
        readme = Some(
            h.materializer
                .bind_working_directory(&id, None)
                .unwrap()
                .root()
                .join("README.md"),
        );
        for operation in ["filesystem.read", "filesystem.write"] {
            child_permissions.push(ScopedPermission {
                operation: operation.into(),
                resource: PermissionResource::Workdir {
                    working_directory_id: id.clone(),
                    path: ".".into(),
                    recursive: true,
                },
            });
        }
        if ticket_mode {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(readme.as_ref().unwrap().parent().unwrap())
                    .args(["switch", "-c", selector])
                    .status()
                    .unwrap()
                    .success()
            );
        }
        context.lock().unwrap().workdir = Some(id);
    } else {
        child_permissions.push(operation("objective.query", true));
    }
    context.lock().unwrap().permissions = child_permissions.clone();
    let store = open_subjektiv_store(&h.api).unwrap();
    let subject = store
        .create_subject(crate::subjektiv::SubjectRole::new("async-chat-delegation").unwrap())
        .unwrap()
        .id;
    let mut execution = [
        "session.record",
        "memory.resident.read",
        "memory.resident.supply",
        "journal.startup.read",
        "journal.startup.supply",
        "chat.read",
        "chat.send",
    ]
    .into_iter()
    .map(|op| subject_permission(&subject, op))
    .collect::<Vec<_>>();
    execution.extend([
        operation("worker.create", true),
        operation("worker.messaging", false),
    ]);
    let mut foreign = None;
    let mut delegation = child_permissions.clone();
    if ticket_mode {
        let mut input = NewTicket::new("Chat requested Ticket work");
        set_test_ticket_target(&mut input, "test-repository", selector);
        let ticket = browser_ticket_backend(&h.api)
            .unwrap()
            .create(input)
            .unwrap()
            .id;
        let other_subject = store
            .create_subject(crate::subjektiv::SubjectRole::new("other-chat-subject").unwrap())
            .unwrap()
            .id;
        let mut input = NewTicket::new("Other Subject Ticket");
        set_test_ticket_target(&mut input, "test-repository", selector);
        let other_ticket = browser_ticket_backend(&h.api)
            .unwrap()
            .create(input)
            .unwrap()
            .id;
        let assigned = account_request(&h,"PUT",&format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{other_ticket}/assignments/orchestrator"),Some(json!({"operation_id":"other-ticket-assign","expected_assignment_id":null,"principal":{"kind":"subject","subject_id":other_subject}}))).await;
        let other_assignment = assigned["assignment"]["assignment_id"].clone();
        for (id, name) in [
            (&ticket, "ticket.read"),
            (&ticket, "ticket.assignment.assign"),
            (&ticket, "ticket.implementation_result.read"),
            (&other_ticket, "ticket.assignment.assign"),
        ] {
            execution.push(ScopedPermission {
                operation: name.into(),
                resource: PermissionResource::Ticket {
                    workspace_id: TEST_WORKSPACE_ID.into(),
                    ticket_id: id.clone(),
                },
            });
        }
        for name in ["ticket.read", "ticket.implementation_result.write"] {
            child_permissions.push(ScopedPermission {
                operation: name.into(),
                resource: PermissionResource::Ticket {
                    workspace_id: TEST_WORKSPACE_ID.into(),
                    ticket_id: ticket.clone(),
                },
            });
        }
        delegation = child_permissions.clone();
        let wrong = account_request(&h,"POST",&format!("/api/w/{TEST_WORKSPACE_ID}/working-directories"),Some(json!({"runtime_id":CONNECTED_RUNTIME,"repository_key":"test-repository","selector":"HEAD","operation_id":"wrong-chat-workdir"}))).await;
        let wrong = wrong["item"]["working_directory_id"].as_str().unwrap();
        let mut wrong_permissions = child_permissions
            .iter()
            .filter(|p| !p.operation.starts_with("filesystem."))
            .cloned()
            .collect::<Vec<_>>();
        for name in ["filesystem.read", "filesystem.write"] {
            let p = ScopedPermission {
                operation: name.into(),
                resource: PermissionResource::Workdir {
                    working_directory_id: wrong.into(),
                    path: ".".into(),
                    recursive: true,
                },
            };
            delegation.push(p.clone());
            wrong_permissions.push(p);
        }
        context.lock().unwrap().permissions = child_permissions.clone();
        context.lock().unwrap().ticket = Some(ticket);
        context.lock().unwrap().denials = vec![
            (
                "chat-denied-other-assignee".into(),
                json!({"ticket_id":other_ticket,"orchestrator_assignment_id":other_assignment}),
                "target Subject".into(),
            ),
            (
                "chat-denied-wrong-target".into(),
                json!({"working_directory_id":wrong,"execution_grant":{"permissions":wrong_permissions,"redelegation":[]}}),
                "selector".into(),
            ),
        ];
        foreign = Some((other_subject, other_ticket));
    }
    account_request(&h,"POST",&format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{subject}/permissions"),Some(json!({"operation_id":"chat-policy","expected_change_id":null,"execution":execution,"delegation":delegation}))).await;
    let original_policy = store.subject_permissions(&subject).unwrap().unwrap();
    for (name, target) in [("conversation-a", true), ("conversation-b", false)] {
        let c = account_request(
            &h,
            "POST",
            &format!("/api/w/{TEST_WORKSPACE_ID}/chat/conversations"),
            Some(json!({"operation_id":name,"subject_id":subject})),
        )
        .await;
        let mut context = context.lock().unwrap();
        if target {
            context.a = c["id"].as_str().unwrap().into();
        } else {
            context.b = c["id"].as_str().unwrap().into();
        }
    }
    let (a, b) = {
        let c = context.lock().unwrap();
        (c.a.clone(), c.b.clone())
    };
    let source = account_request(&h,"POST",&format!("/api/w/{TEST_WORKSPACE_ID}/workers"),Some(json!({"runtime_id":CONNECTED_RUNTIME,"display_name":"Restricted async chat","profile":"project:async-chat","initial_submit":[],"workdir_attachments":[],"feature_connections":{"subjektiv":{"subject_id":subject}}}))).await;
    let source = RuntimeWorkerRef::new(
        source["runtime_id"].as_str().unwrap(),
        source["worker_id"].as_str().unwrap(),
    );
    if let Some(ticket) = context.lock().unwrap().ticket.clone() {
        account_request(&h,"PUT",&format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator"),Some(json!({"operation_id":"chat-ticket-assign","expected_assignment_id":null,"principal":{"kind":"subject","subject_id":subject}}))).await;
    }
    let mut events = h.runtime.subscribe_worker_observation().unwrap();
    let first = account_request(&h,"POST",&messages(&a),Some(json!({"operation_id":"request-a","text":"Please perform bounded background work for A"}))).await;
    assert_eq!(first["delivery"]["state"], "accepted");
    tokio::time::timeout(std::time::Duration::from_secs(60), start.recv())
        .await
        .expect("real child started")
        .unwrap();
    idle(&mut events, &source.worker_id, &body_phase, 5).await;
    assert_eq!(
        body_phase.load(Ordering::SeqCst),
        5,
        "body finishes rather than polling/waiting for work"
    );
    assert_eq!(
        h.runtime.list_workers().unwrap().len(),
        2,
        "same public Spawn operation retains one child"
    );
    if let Some(path) = &readme {
        assert_eq!(fs::read_to_string(path).unwrap(), "clean\n");
    }
    assert_eq!(
        account_request(&h, "GET", &messages(&a), None).await["messages"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "creation/assistant output does not publish a result"
    );
    let second = account_request(&h,"POST",&messages(&b),Some(json!({"operation_id":"request-b","text":"A separate question B while background work continues"}))).await;
    assert_eq!(second["delivery"]["state"], "accepted");
    idle(&mut events, &source.worker_id, &body_phase, 8).await;
    let page_b = account_request(&h, "GET", &messages(&b), None).await;
    assert_eq!(page_b["messages"].as_array().unwrap().len(), 2);
    assert_eq!(page_b["messages"][1]["text"], PUBLIC_B);
    assert_eq!(page_b["messages"][1]["reply_to"], second["message"]["id"]);
    assert_eq!(
        account_request(&h, "GET", &messages(&a), None).await["messages"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    release.add_permits(1);
    tokio::time::timeout(std::time::Duration::from_secs(60), private_input.recv())
        .await
        .expect("private report model boundary")
        .unwrap();
    assert_eq!(
        account_request(&h, "GET", &messages(&a), None).await["messages"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "ordinary WorkerNotify is not public chat output"
    );
    let source_snapshot = h
        .factory
        .retained_session_snapshot(&worker_runtime::identity::WorkerRef::try_from(&source).unwrap())
        .unwrap();
    assert_eq!(source_snapshot.snapshot.entries.iter().filter(|entry| matches!(&entry.data, protocol::SessionSnapshotEntryData::SystemItem {item_kind,content,..} if item_kind == "notification" && content.contains(REPORT))).count(),1,"ordinary report committed exactly once before model supply");
    if let Some(path) = &readme {
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "background changed after real Read\n"
        );
    }
    if reassign_before_evaluation {
        let ticket = context.lock().unwrap().ticket.clone().unwrap();
        let current = h
            .api
            .store
            .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket)
            .unwrap()
            .remove(0);
        let (other, _) = foreign.as_ref().unwrap();
        account_request(&h,"PUT",&format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator"),Some(json!({"operation_id":"reassign-before-chat-result-evaluation","expected_assignment_id":current.assignment_id,"principal":{"kind":"subject","subject_id":other}}))).await;
    }
    release_send.add_permits(1);
    tokio::time::timeout(std::time::Duration::from_secs(60), completion.recv())
        .await
        .expect("ordinary report and explicit A send")
        .unwrap();
    idle(&mut events, &source.worker_id, &body_phase, 11).await;
    let page_a = account_request(&h, "GET", &messages(&a), None).await;
    if reassign_before_evaluation {
        assert_eq!(
            page_a["messages"].as_array().unwrap().len(),
            1,
            "old report does not automatically publish or grant a new Ticket attempt"
        );
    } else {
        assert_eq!(page_a["messages"].as_array().unwrap().len(), 2);
        assert_eq!(page_a["messages"][1]["text"], PUBLIC_A);
        assert_eq!(page_a["messages"][1]["reply_to"], first["message"]["id"]);
    }
    assert_eq!(
        account_request(&h, "GET", &messages(&b), None).await,
        page_b,
        "A's report never crosses into B"
    );
    assert!(!page_a.to_string().contains(REPORT) && !page_a.to_string().contains("PRIVATE_CHAT"));
    if let Some(path) = &readme {
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "background changed after real Read\n"
        );
    }
    assert_eq!(
        store.subject_permissions(&subject).unwrap().unwrap(),
        original_policy,
        "no per-target Subject policy copy"
    );
    let child = context.lock().unwrap().spawned.clone().unwrap();
    let forward = h
        .api
        .store
        .get_active_worker_control_grant(TEST_WORKSPACE_ID, &source, &child)
        .unwrap()
        .unwrap();
    let reverse = h
        .api
        .store
        .get_active_worker_control_grant(TEST_WORKSPACE_ID, &child, &source)
        .unwrap()
        .unwrap();
    assert_eq!(reverse.permissions, vec!["notify".to_owned()]);
    assert!(
        h.api
            .store
            .require_current_worker_singleton_owner(TEST_WORKSPACE_ID, &child)
            .unwrap()
            .is_none()
    );
    let grant = store
        .worker_execution_grant(&child.runtime_id, &child.worker_id)
        .unwrap()
        .unwrap();
    assert_eq!(grant.permissions, context.lock().unwrap().permissions);
    if ticket_mode {
        let ticket = context.lock().unwrap().ticket.clone().unwrap();
        assert_eq!(
            h.api
                .store
                .get_current_ticket_worker_assignment(TEST_WORKSPACE_ID, &ticket)
                .unwrap()
                .unwrap()
                .worker,
            child
        );
        let results = h.api.config_store.with_conn(|conn| {
            Ok(conn.query_row("SELECT COUNT(*) FROM ticket_implementation_results WHERE workspace_id=?1 AND ticket_id=?2",rusqlite::params![TEST_WORKSPACE_ID,ticket],|row|row.get::<_,i64>(0))?)
        }).unwrap();
        assert_eq!(
            results, 1,
            "one canonical Ticket result, no parallel result ledger"
        );
        assert_eq!(
            h.api.authority.ticket(&ticket).unwrap().state,
            server_api::BrowserTicketState::Open
        );
        let current = h
            .api
            .store
            .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket)
            .unwrap()
            .remove(0);
        assert_eq!(
            current.principal,
            ticket::TicketAssignmentPrincipalSnapshot::Subject {
                subject_id: if reassign_before_evaluation {
                    foreign.as_ref().unwrap().0.clone()
                } else {
                    subject.clone()
                }
            }
        );
        assert_eq!(
            current.tracking.unwrap().status,
            ticket::TicketAssignmentTrackingStatus::Waiting
        );
        let (other, other_ticket) = foreign.as_ref().unwrap();
        assert_ne!(&subject, other);
        assert!(
            h.api
                .store
                .get_current_ticket_worker_assignment(TEST_WORKSPACE_ID, other_ticket)
                .unwrap()
                .is_none(),
            "wrong Subject request has no Ticket execution effect"
        );
        assert_eq!(
            h.runtime.list_workers().unwrap().len(),
            2,
            "denials/retries allocate no extra Worker"
        );
    }
    assert!(
        !grant
            .permissions
            .iter()
            .any(|permission| permission.operation == "chat.send")
    );
    assert!(
        h.api
            .store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, &source)
            .unwrap()
            .is_empty()
    );
    if explicit_workdir {
        h.api
            .store
            .revoke_worker_control_grant(
                TEST_WORKSPACE_ID,
                &forward.grant_id,
                &now_registry_timestamp(),
            )
            .unwrap();
    } else {
        let narrowed = execution
            .into_iter()
            .filter(|permission| permission.operation != "worker.messaging")
            .collect::<Vec<_>>();
        account_request(&h,"POST",&format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{subject}/permissions"),Some(json!({"operation_id":"revoke-messaging","expected_change_id":original_policy.change_id,"execution":narrowed,"delegation":context.lock().unwrap().permissions}))).await;
    }
    account_request(&h,"POST",&messages(&a),Some(json!({"operation_id":"request-denied","text":"Attempt follow-up after authorization is revoked"}))).await;
    tokio::time::timeout(std::time::Duration::from_secs(60), completion.recv())
        .await
        .expect("public refusal after revocation")
        .unwrap();
    idle(&mut events, &source.worker_id, &body_phase, 13).await;
    let snapshot = h
        .factory
        .retained_session_snapshot(&worker_runtime::identity::WorkerRef::try_from(&child).unwrap())
        .unwrap();
    assert!(
        !serde_json::to_string(&snapshot.snapshot)
            .unwrap()
            .contains("MUST_NOT_REACH_REVOKED_CHILD")
    );
    assert_eq!(
        account_request(&h, "GET", &messages(&a), None).await["messages"]
            .as_array()
            .unwrap()
            .len(),
        if reassign_before_evaluation { 2 } else { 3 },
        "failed Notify creates no automatic success publication"
    );
    assert_eq!(dropped.load(Ordering::SeqCst), lose_spawn_ack);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn restricted_chat_delegates_real_file_work_answers_b_then_explicitly_returns_to_a() {
    connected_chat_delegation(Scenario::Workdir).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn restricted_workdirless_chat_delegates_bounded_query_and_refuses_revoked_messaging() {
    connected_chat_delegation(Scenario::Workdirless).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn restricted_chat_uses_current_ticket_assignee_target_and_saved_result_before_publication() {
    connected_chat_delegation(Scenario::Ticket).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn reassignment_during_private_chat_result_evaluation_refuses_old_report_as_execution_authority()
 {
    connected_chat_delegation(Scenario::ReassignedTicket).await;
}
