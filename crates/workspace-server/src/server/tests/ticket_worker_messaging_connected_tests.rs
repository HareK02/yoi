//! T778's Ticket consumer on the selected T803 public delegation contract.
//! Only the external model is scripted. Shared signed Backend/Runtime harness,
//! Controllers, public Tools, policy/grant ledgers, Workdirs and results are real.
//! Acceptance alone is not execution: completion requires Read/Write/Read/result
//! and an actual public Notify reply observed by the originating model.
use super::*;
use agen::llm_client::{
    event::{Event, StopReason},
    types::{ContentPart, Item},
};
use worker_runtime::worker_backend::RuntimeWorkerFactory;

const ORIGIN: &str = "t778-public-origin:";
const CHILD: &str = "t778-public-child:";
const REPLY: &str = "T778_PUBLIC_REPLY:";
const REVOCATION: &str = "t778-public-revocation:";

fn operation(name: &str) -> ScopedPermission {
    ScopedPermission {
        operation: name.into(),
        resource: PermissionResource::WorkerOperation {
            workspace_id: TEST_WORKSPACE_ID.into(),
        },
    }
}
fn collection(name: &str) -> ScopedPermission {
    ScopedPermission {
        operation: name.into(),
        resource: PermissionResource::Workspace {
            workspace_id: TEST_WORKSPACE_ID.into(),
        },
    }
}
fn ticket_permission(ticket: &str, name: &str) -> ScopedPermission {
    ScopedPermission {
        operation: name.into(),
        resource: PermissionResource::Ticket {
            workspace_id: TEST_WORKSPACE_ID.into(),
            ticket_id: ticket.into(),
        },
    }
}
fn file_permissions(workdir: &str) -> Vec<ScopedPermission> {
    ["filesystem.read", "filesystem.write"]
        .into_iter()
        .map(|operation| ScopedPermission {
            operation: operation.into(),
            resource: PermissionResource::Workdir {
                working_directory_id: workdir.into(),
                path: "README.md".into(),
                recursive: false,
            },
        })
        .collect()
}
fn config(request: &agen::llm_client::Request, prefix: &str) -> Option<Value> {
    request.items.iter().find_map(|item| {
        let Item::Message { content, .. } = item else {
            return None;
        };
        content.iter().find_map(|part| match part {
            // Notify is rendered through the production notify_wrapper prompt;
            // parse the first JSON value after our marker, not the wrapper suffix.
            ContentPart::Text { text } => text.find(prefix).map(|offset| {
                serde_json::Deserializer::from_str(&text[offset + prefix.len()..])
                    .into_iter::<Value>()
                    .next()
                    .expect("script marker JSON")
                    .expect("valid script marker JSON")
            }),
            _ => None,
        })
    })
}
fn tool_result(request: &agen::llm_client::Request, id: &str) -> Option<(bool, String)> {
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
fn success(request: &agen::llm_client::Request, id: &str) -> Option<Value> {
    tool_result(request, id).map(|(error, content)| {
        assert!(!error, "public {id} refused: {content}");
        serde_json::from_str(&content).expect("public JSON Tool result")
    })
}
fn refused(request: &agen::llm_client::Request, id: &str) -> bool {
    tool_result(request, id).is_some_and(|(error, content)| {
        assert!(error, "public {id} must refuse: {content}");
        true
    })
}
fn call(request: &agen::llm_client::Request, id: &str, name: &str, input: Value) -> Vec<Event> {
    assert!(
        request.tools.iter().any(|tool| tool.name == name),
        "Profile must expose {name}; absence is not authorization evidence"
    );
    vec![
        Event::tool_use_start(0, id, name),
        Event::tool_input_delta(0, input.to_string()),
        Event::tool_use_stop(0),
    ]
}
fn done() -> Vec<Event> {
    vec![
        Event::text_block_start(0),
        Event::text_delta(0, "scripted public Ticket turn finished"),
        Event::text_block_stop(0, Some(StopReason::EndTurn)),
    ]
}
fn target(worker: &RuntimeWorkerRef) -> Value {
    json!({"kind":"runtime_worker", "runtime_id":worker.runtime_id, "worker_id":worker.worker_id})
}

fn notifications(snapshot: &protocol::SessionSnapshot) -> Vec<&str> {
    // Outgoing refused ToolCall arguments are ALSO canonical history. They are
    // not evidence that the destination accepted an incoming notification.
    snapshot
        .entries
        .iter()
        .filter_map(|entry| match &entry.data {
            protocol::SessionSnapshotEntryData::SystemItem {
                item_kind, content, ..
            } if item_kind == "notification" => Some(content.as_str()),
            _ => None,
        })
        .collect()
}

#[derive(Clone)]
struct PublicTicketModel {
    admission: Arc<tokio::sync::Barrier>,
    overlap: Arc<tokio::sync::Barrier>,
    replied: Arc<Mutex<std::collections::BTreeMap<String, Arc<tokio::sync::Notify>>>>,
    completed: tokio::sync::mpsc::Sender<Value>,
    recorded: Arc<Mutex<std::collections::BTreeSet<String>>>,
}
impl PublicTicketModel {
    async fn origin(&self, request: &agen::llm_client::Request, input: Value) -> Vec<Event> {
        let ticket = input["ticket"].as_str().unwrap();
        // These are ordinary public Tool calls. No signed helper substitutes for
        // the subject Controller's Spawn authorization or WorkerNotify dispatch.
        for denial in input["denials"].as_array().unwrap() {
            let id = denial["id"].as_str().unwrap();
            if !refused(request, id) {
                return call(request, id, "WorkerSpawn", denial["input"].clone());
            }
            if let Some(reason) = denial["reason"].as_str() {
                assert!(
                    tool_result(request, id).unwrap().1.contains(reason),
                    "{id} must exercise the intended {reason} fence, not another refusal: {:?}",
                    tool_result(request, id)
                );
            }
        }
        let Some(spawn) = success(request, "public-ticket-spawn") else {
            // Neither peer may acquire its valid Ticket execution assignment
            // before both wrong-assignee probes reach the intended Subject fence.
            tokio::time::timeout(std::time::Duration::from_secs(40), self.admission.wait())
                .await
                .expect("both public refusal sequences completed");
            return call(
                request,
                "public-ticket-spawn",
                "WorkerSpawn",
                input["spawn"].clone(),
            );
        };
        let Some(replay) = success(request, "public-ticket-replay") else {
            // Exact same caller intent and full public input, not a new operation.
            return call(
                request,
                "public-ticket-replay",
                "WorkerSpawn",
                input["spawn"].clone(),
            );
        };
        assert_eq!(spawn["runtime_id"], replay["runtime_id"]);
        assert_eq!(spawn["worker_id"], replay["worker_id"]);
        for (id, subject) in [
            ("public-cross-subject-notify", input["other_origin"].clone()),
            (
                "public-invalid-target-notify",
                json!({"kind":"runtime_worker", "runtime_id":CONNECTED_RUNTIME,"worker_id":"t778-unknown-target"}),
            ),
        ] {
            if !refused(request, id) {
                return call(
                    request,
                    id,
                    "WorkerNotify",
                    json!({"subject":subject,"content":"T778_MUST_NOT_ACCEPT_FOREIGN_NOTIFY"}),
                );
            }
        }
        let messages = request
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Message { content, .. } => Some(serde_json::to_string(content).unwrap()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        if let Some(reply) = config(request, REPLY) {
            assert_eq!(
                reply["ticket"], ticket,
                "another Ticket's reply cannot finish this body"
            );
            let Some(evaluated) = success(request, "public-origin-evaluate-result") else {
                return call(
                    request,
                    "public-origin-evaluate-result",
                    "TicketImplementationResultQuery",
                    json!({"ticket":ticket,"result_id":reply["result_id"]}),
                );
            };
            assert_eq!(evaluated["ticket_id"], ticket);
            assert_eq!(evaluated["result_id"], reply["result_id"]);
            assert_eq!(
                evaluated["request"]["body"],
                format!(
                    "Public tools verified {}",
                    input["child"]["content"].as_str().unwrap()
                )
            );
            if self.recorded.lock().unwrap().insert(ticket.into()) {
                self.completed
                    .try_send(
                        json!({"ticket":ticket,"spawn":spawn,"replay":replay,"messages":messages,"evaluated":evaluated}),
                    )
                    .unwrap();
            }
            return done();
        }
        if let Some(ping) = success(request, "public-ticket-ping") {
            assert_eq!(ping["state"], "accepted", "{ping}");
            let signal = self.replied.lock().unwrap().get(ticket).unwrap().clone();
            tokio::time::timeout(std::time::Duration::from_secs(40), signal.notified())
                .await
                .expect("real child public reply result");
            return done();
        }
        call(
            request,
            "public-ticket-ping",
            "WorkerNotify",
            json!({
                "subject":{"kind":"runtime_worker", "runtime_id":spawn["runtime_id"],"worker_id":spawn["worker_id"]},
                "content":format!("{CHILD}{}",input["child"])
            }),
        )
    }
    async fn child(&self, request: &agen::llm_client::Request, input: Value) -> Vec<Event> {
        let ticket = input["ticket"].as_str().unwrap();
        if let Some(reply) = success(request, "public-child-reply") {
            assert_eq!(reply["state"], "accepted", "{reply}");
            self.replied
                .lock()
                .unwrap()
                .get(ticket)
                .unwrap()
                .notify_one();
            return done();
        }
        if let Some(receipt) = success(request, "public-child-result") {
            if !refused(request, "public-child-foreign-result") {
                return call(
                    request,
                    "public-child-foreign-result",
                    "TicketImplementationResult",
                    json!({"ticket":input["other_ticket"],"kind":"result","body":"T778_MUST_NOT_SAVE_FOREIGN_RESULT"}),
                );
            }
            if !refused(request, "public-child-foreign-notify") {
                return call(
                    request,
                    "public-child-foreign-notify",
                    "WorkerNotify",
                    json!({"subject":input["other_origin"],"content":"T778_MUST_NOT_ACCEPT_FOREIGN_NOTIFY"}),
                );
            }
            return call(
                request,
                "public-child-reply",
                "WorkerNotify",
                json!({"subject":input["origin"],"content":format!("{REPLY}{}",json!({"ticket":ticket,"result_id":receipt["result_id"]}))}),
            );
        }
        if let Some((error, reread)) = tool_result(request, "public-child-reread") {
            assert!(!error, "post-write Read: {reread}");
            assert!(
                reread.contains(input["content"].as_str().unwrap()),
                "{reread}"
            );
            return call(
                request,
                "public-child-result",
                "TicketImplementationResult",
                json!({"ticket":ticket,"kind":"result","body":format!("Public tools verified {}",input["content"].as_str().unwrap())}),
            );
        }
        if let Some((error, content)) = tool_result(request, "public-child-write") {
            assert!(!error, "real Write: {content}");
            return call(
                request,
                "public-child-reread",
                "Read",
                json!({"target_workdir":"workdir","file_path":"README.md"}),
            );
        }
        if let Some((error, read)) = tool_result(request, "public-child-read") {
            assert!(
                !error && read.contains("clean"),
                "actual initial Read: {read}"
            );
            tokio::time::timeout(std::time::Duration::from_secs(40), self.overlap.wait())
                .await
                .expect("two public delegated Controllers overlap after Read");
            return call(
                request,
                "public-child-write",
                "Write",
                json!({"target_workdir":"workdir","file_path":"README.md","content":input["content"]}),
            );
        }
        if !refused(request, "public-child-private-read") {
            return call(
                request,
                "public-child-private-read",
                "Read",
                json!({"target_workdir":"workdir","file_path":"private.txt"}),
            );
        }
        call(
            request,
            "public-child-read",
            "Read",
            json!({"target_workdir":"workdir","file_path":"README.md"}),
        )
    }
}
#[async_trait::async_trait]
impl agen::llm_client::LlmClient for PublicTicketModel {
    fn clone_boxed(&self) -> Box<dyn agen::llm_client::LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        request: agen::llm_client::Request,
    ) -> std::result::Result<agen::llm_client::client::ResponseStream, agen::llm_client::ClientError>
    {
        let events = if let Some(input) = config(&request, REVOCATION) {
            if !refused(&request, "revoked-public-spawn") {
                call(
                    &request,
                    "revoked-public-spawn",
                    "WorkerSpawn",
                    input["spawn"].clone(),
                )
            } else if !refused(&request, "revoked-public-notify") {
                call(
                    &request,
                    "revoked-public-notify",
                    "WorkerNotify",
                    json!({
                        "subject":input["child"],"content":"T778_MUST_NOT_ACCEPT_REVOKED_NOTIFY"
                    }),
                )
            } else {
                let key = format!("revocation:{}", input["ticket"].as_str().unwrap());
                if self.recorded.lock().unwrap().insert(key) {
                    self.completed
                        .try_send(json!({"revocation":input["ticket"]}))
                        .unwrap();
                }
                done()
            }
        } else if let Some(input) = config(&request, ORIGIN) {
            self.origin(&request, input).await
        } else if let Some(input) = config(&request, CHILD) {
            self.child(&request, input).await
        } else {
            // Normal startup/assignment lifecycle input is not the child script.
            done()
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}

fn install_public_ticket_profiles(api: &WorkspaceApi) {
    let state = api
        .config_store
        .load_workspace_config(TEST_WORKSPACE_ID)
        .unwrap()
        .unwrap();
    let main = state
        .snapshot
        .get(&config_source::VirtualPath::parse("main.dcdl").unwrap())
        .unwrap();
    let content = r#"{ profile = { default_profile = "project:public-ticket-origin"; entries = [
        { selector = "project:public-ticket-origin"; label = "Ticket Subject origin"; description = "Public Ticket consumer";
          profile = (import "$builtin/profiles/companion.dcdl") // { slug = "public-ticket-origin"; scope = "workspace_read"; delegation_scope = "workspace_read"; feature = { worker = { enabled = true; direct_spawn = true; }; }; };
        },
        { selector = "project:public-ticket-editor"; label = "Ticket delegated editor"; description = "Explicit grants only";
          profile = (import "$builtin/profiles/coder.dcdl") // { slug = "public-ticket-editor"; scope = "workspace_write"; feature = { subjektiv = { enabled = false; }; worker = { enabled = true; direct_spawn = false; }; }; };
        }
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

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn public_two_subject_ticket_spawn_notify_edit_result_retry_and_fences() {
    use worker_runtime::working_directory::WorkingDirectoryMaterializer;
    struct Case {
        subject: String,
        ticket: String,
        assignment: String,
        branch: String,
        workdir: String,
        root: PathBuf,
        origin: RuntimeWorkerRef,
        permissions: Vec<ScopedPermission>,
        execution: Vec<ScopedPermission>,
        change: String,
        spawn: Value,
        content: String,
    }
    let (completed, mut completion) = tokio::sync::mpsc::channel(2);
    let replied = Arc::new(Mutex::new(std::collections::BTreeMap::new()));
    let harness = ConnectedWorkerHarness::start(PublicTicketModel {
        admission: Arc::new(tokio::sync::Barrier::new(2)),
        overlap: Arc::new(tokio::sync::Barrier::new(2)),
        replied: replied.clone(),
        completed,
        recorded: Arc::new(Mutex::new(std::collections::BTreeSet::new())),
    })
    .await;
    let api = &harness.api;
    install_public_ticket_profiles(api);
    let subjects = open_subjektiv_store(api).unwrap();
    // Deliberately permitted resource, but wrong Ticket branch. This separates a
    // target rejection from generic delegation-scope rejection.
    let (status, wrong) = harness.human(reqwest::Method::POST,
        &format!("/api/w/{TEST_WORKSPACE_ID}/working-directories"),
        json!({"runtime_id":CONNECTED_RUNTIME,"repository_key":"test-repository","selector":"HEAD","operation_id":"public-ticket-wrong-target-workdir"})).await;
    assert_eq!(status, StatusCode::CREATED, "{wrong}");
    let wrong_workdir = wrong["item"]["working_directory_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut cases = Vec::new();
    for label in ["alpha", "beta"] {
        let branch = format!("public-ticket-{label}");
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(harness._dir.path())
                .args(["branch", &branch])
                .status()
                .unwrap()
                .success()
        );
        let (status, created) = harness.human(reqwest::Method::POST,
            &format!("/api/w/{TEST_WORKSPACE_ID}/working-directories"),
            json!({"runtime_id":CONNECTED_RUNTIME,"repository_key":"test-repository","selector":branch,"operation_id":format!("public-{label}-workdir")})).await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let workdir = created["item"]["working_directory_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let root = harness
            .materializer
            .bind_working_directory(&workdir, None)
            .unwrap()
            .root()
            .to_path_buf();
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["switch", "-c", &branch])
                .status()
                .unwrap()
                .success()
        );
        fs::write(root.join("private.txt"), "PRIVATE_MUST_NOT_READ\n").unwrap();
        let subject = subjects
            .create_subject(
                crate::subjektiv::SubjectRole::new(format!("public-ticket-{label}")).unwrap(),
            )
            .unwrap()
            .id;
        let mut input = NewTicket::new(format!("Public connected {label} Ticket"));
        set_test_ticket_target(&mut input, "test-repository", &branch);
        let ticket = browser_ticket_backend(api)
            .unwrap()
            .create(input)
            .unwrap()
            .id;
        replied
            .lock()
            .unwrap()
            .insert(ticket.clone(), Arc::new(tokio::sync::Notify::new()));
        let mut permissions = file_permissions(&workdir);
        permissions.extend([
            ticket_permission(&ticket, "ticket.read"),
            ticket_permission(&ticket, "ticket.implementation_result.write"),
            operation("worker.messaging"),
        ]);
        let mut delegation = permissions.clone();
        delegation.extend(file_permissions(&wrong_workdir));
        let mut execution: Vec<_> = [
            "session.record",
            "memory.resident.read",
            "memory.resident.supply",
            "journal.startup.read",
            "journal.startup.supply",
        ]
        .into_iter()
        .map(|name| subject_permission(&subject, name))
        .collect();
        execution.extend([
            collection("worker.create"),
            operation("worker.messaging"),
            ticket_permission(&ticket, "ticket.assignment.assign"),
            ticket_permission(&ticket, "ticket.read"),
            ticket_permission(&ticket, "ticket.implementation_result.read"),
        ]);
        let (status, policy) = harness.human(reqwest::Method::POST,
            &format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{subject}/permissions"),
            json!({"operation_id":format!("public-{label}-policy"),"expected_change_id":null,"execution":execution,"delegation":delegation})).await;
        assert_eq!(status, StatusCode::OK, "{policy}");
        let change = subjects
            .subject_permissions(&subject)
            .unwrap()
            .unwrap()
            .change_id;
        let (status, source) = harness.human(reqwest::Method::POST,
            &format!("/api/w/{TEST_WORKSPACE_ID}/workers"),
            json!({"runtime_id":CONNECTED_RUNTIME,"display_name":format!("{label} Ticket Subject"),"profile":"project:public-ticket-origin","initial_submit":[],"workdir_attachments":[],"feature_connections":{"subjektiv":{"subject_id":subject}}})).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{source}; Runtime: {:?}",
            harness.runtime_errors.lock().unwrap()
        );
        let origin = RuntimeWorkerRef::new(
            source["runtime_id"].as_str().unwrap(),
            source["worker_id"].as_str().unwrap(),
        );
        let (status, assigned) = harness.human(reqwest::Method::PUT,
            &format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator"),
            json!({"operation_id":format!("public-{label}-assign"),"expected_assignment_id":null,"principal":{"kind":"subject","subject_id":subject}})).await;
        assert_eq!(status, StatusCode::OK, "{assigned}");
        let assignment = assigned["assignment"]["assignment_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let content = format!("public connected {label} verified\n");
        let spawn = json!({"runtime_id":CONNECTED_RUNTIME,"profile":"project:public-ticket-editor","operation_id":format!("public-{label}-spawn"),
            "display_name":format!("{label} public Ticket editor"),"ticket_id":ticket,"orchestrator_assignment_id":assignment,
            "working_directory_id":workdir,"initial_submit":[],"execution_grant":{"permissions":permissions,"redelegation":[]}});
        cases.push(Case {
            subject,
            ticket,
            assignment,
            branch,
            workdir,
            root,
            origin,
            permissions,
            execution,
            change,
            spawn,
            content,
        });
    }
    assert_ne!(cases[0].subject, cases[1].subject);
    assert_ne!(cases[0].ticket, cases[1].ticket);
    assert_ne!(cases[0].workdir, cases[1].workdir);
    assert_ne!(cases[0].root, cases[1].root);
    // Permit the generic start operation on the peer Ticket as well. Its current
    // assigned Subject must STILL refuse this caller, independently of operation
    // permission and the caller's valid explicit child execution grant.
    for i in 0..2 {
        let peer_ticket = cases[1 - i].ticket.clone();
        let case = &mut cases[i];
        case.execution
            .push(ticket_permission(&peer_ticket, "ticket.assignment.assign"));
        let previous = subjects
            .subject_permissions(&case.subject)
            .unwrap()
            .unwrap();
        let (status, body) = harness.human(reqwest::Method::POST,
            &format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{}/permissions", case.subject),
            json!({"operation_id":format!("public-peer-start-operation-{i}"), "expected_change_id":previous.change_id,
                "execution":case.execution,"delegation":previous.delegation})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        case.change = subjects
            .subject_permissions(&case.subject)
            .unwrap()
            .unwrap()
            .change_id;
    }
    for (i, case) in cases.iter().enumerate() {
        let other = &cases[1 - i];
        let mut assignee = case.spawn.clone();
        assignee["operation_id"] = json!(format!("public-denied-assignee-{i}"));
        assignee["ticket_id"] = json!(other.ticket);
        assignee["orchestrator_assignment_id"] = json!(other.assignment);
        let mut wrong_target = case.spawn.clone();
        wrong_target["operation_id"] = json!(format!("public-denied-target-{i}"));
        wrong_target["working_directory_id"] = json!(wrong_workdir);
        let mut wrong_permissions = case.permissions[2..].to_vec();
        wrong_permissions.extend(file_permissions(&wrong_workdir));
        wrong_target["execution_grant"]["permissions"] = json!(wrong_permissions);
        let mut resource = case.spawn.clone();
        resource["operation_id"] = json!(format!("public-denied-resource-{i}"));
        let mut cross_permissions = case.permissions.clone();
        cross_permissions.extend(file_permissions(&other.workdir));
        resource["execution_grant"]["permissions"] = json!(cross_permissions);
        let input = json!({"ticket":case.ticket,"spawn":case.spawn,"other_origin":target(&other.origin),
            "denials":[{"id":"public-wrong-assignee-spawn","input":assignee,"reason":"target Subject"},
                {"id":"public-wrong-target-spawn","input":wrong_target,"reason":"selector"},
                {"id":"public-cross-resource-spawn","input":resource}],
            "child":{"ticket":case.ticket,"other_ticket":other.ticket,"content":case.content,
                "origin":target(&case.origin),"other_origin":target(&other.origin)}});
        // Human Notify only starts the already-created Subject body (also safe
        // while assignment attention is Busy). Child creation, retry, messaging
        // and editing below occur through public Tools.
        let (status, body) = harness
            .human(
                reqwest::Method::POST,
                &format!(
                    "/api/w/{TEST_WORKSPACE_ID}/runtimes/{}/workers/{}/input",
                    case.origin.runtime_id, case.origin.worker_id
                ),
                json!({"kind":"notify","content":format!("{ORIGIN}{input}")}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let mut observed = std::collections::BTreeMap::new();
    for _ in 0..2 {
        let value = tokio::time::timeout(std::time::Duration::from_secs(90), completion.recv())
            .await
            .expect("two real public Ticket roundtrips")
            .expect("model completion evidence");
        let ticket = value["ticket"].as_str().unwrap().to_owned();
        assert!(observed.insert(ticket, value).is_none());
    }
    assert_eq!(
        harness.runtime.list_workers().unwrap().len(),
        4,
        "two bodies + exactly two children; public replay and refusals allocate no extra child"
    );
    let saved = api.config_store.with_conn(|conn| {
        let mut statement = conn.prepare("SELECT ticket_id,worker_id,result_id FROM ticket_implementation_results WHERE workspace_id=?1 ORDER BY ticket_id")?;
        Ok(statement.query_map([TEST_WORKSPACE_ID],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?)))?.collect::<std::result::Result<Vec<_>,_>>()?)
    }).unwrap();
    assert_eq!(
        saved.len(),
        2,
        "foreign public result requests must not persist"
    );
    let mut children = Vec::new();
    for (i, case) in cases.iter().enumerate() {
        let spawn = &observed[&case.ticket]["spawn"];
        let child = RuntimeWorkerRef::new(
            spawn["runtime_id"].as_str().unwrap(),
            spawn["worker_id"].as_str().unwrap(),
        );
        let grant = subjects
            .worker_execution_grant(&child.runtime_id, &child.worker_id)
            .unwrap()
            .unwrap();
        assert_eq!(grant.subject_id, case.subject);
        assert_eq!(grant.origin_worker_id, case.origin.worker_id);
        assert_eq!(grant.source_change_id, case.change);
        assert_eq!(grant.permissions, case.permissions);
        assert!(grant.redelegation.is_empty());
        let forward = api
            .store
            .get_active_worker_control_grant(TEST_WORKSPACE_ID, &case.origin, &child)
            .unwrap()
            .unwrap();
        assert!(forward.permissions.contains(&"notify".into()));
        let reverse = api
            .store
            .get_active_worker_control_grant(TEST_WORKSPACE_ID, &child, &case.origin)
            .unwrap()
            .unwrap();
        assert_eq!(reverse.permissions, vec!["notify".to_owned()]);
        assert!(
            api.store
                .get_active_worker_control_grant(TEST_WORKSPACE_ID, &child, &cases[1 - i].origin)
                .unwrap()
                .is_none()
        );
        assert!(
            api.store
                .list_worker_workdir_links(TEST_WORKSPACE_ID, &case.origin)
                .unwrap()
                .is_empty()
        );
        let links = api
            .store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, &child)
            .unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].workdir_id, case.workdir);
        assert_eq!(
            api.store
                .get_current_ticket_worker_assignment(TEST_WORKSPACE_ID, &case.ticket)
                .unwrap()
                .unwrap()
                .worker,
            child
        );
        assert_eq!(
            fs::read_to_string(case.root.join("README.md")).unwrap(),
            case.content
        );
        assert_eq!(
            fs::read_to_string(case.root.join("private.txt")).unwrap(),
            "PRIVATE_MUST_NOT_READ\n"
        );
        let branch = std::process::Command::new("git")
            .arg("-C")
            .arg(&case.root)
            .args(["branch", "--show-current"])
            .output()
            .unwrap();
        assert!(branch.status.success());
        assert_eq!(
            String::from_utf8(branch.stdout).unwrap().trim(),
            case.branch
        );
        let row = saved.iter().find(|row| row.0 == case.ticket).unwrap();
        assert_eq!(
            observed[&case.ticket]["evaluated"]["result_id"], row.2,
            "origin evaluated the same canonical result that the child persisted"
        );
        assert_eq!(row.1, child.worker_id);
        let receipt = api
            .config_store
            .implementation_result(TEST_WORKSPACE_ID, &case.ticket, &row.2)
            .unwrap();
        assert_eq!(
            receipt.request.body,
            format!("Public tools verified {}", case.content)
        );
        let current = api
            .store
            .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, &case.ticket)
            .unwrap()
            .remove(0);
        assert_eq!(current.assignment_id, case.assignment);
        assert_eq!(
            current.principal,
            ticket::TicketAssignmentPrincipalSnapshot::Subject {
                subject_id: case.subject.clone()
            }
        );
        assert_eq!(
            current.tracking.unwrap().status,
            ticket::TicketAssignmentTrackingStatus::Waiting,
            "result is not Subject handling or case closure"
        );
        assert_eq!(
            api.authority.ticket(&case.ticket).unwrap().state,
            server_api::BrowserTicketState::Open
        );
        let worker = worker_runtime::identity::WorkerRef::try_from(&child).unwrap();
        let snapshot = harness.factory.retained_session_snapshot(&worker).unwrap();
        let history = serde_json::to_string(&snapshot.snapshot).unwrap();
        let incoming = notifications(&snapshot.snapshot);
        assert!(
            incoming.iter().any(|content| content.contains(CHILD)),
            "real Idle Notify reached canonical notification history"
        );
        assert!(
            incoming
                .iter()
                .all(|content| !content.contains("T778_MUST_NOT_ACCEPT_FOREIGN_NOTIFY")),
            "foreign Notify must not become an incoming notification"
        );
        let origin_ref = worker_runtime::identity::WorkerRef::try_from(&case.origin).unwrap();
        let origin_snapshot = harness
            .factory
            .retained_session_snapshot(&origin_ref)
            .unwrap();
        let origin_incoming = notifications(&origin_snapshot.snapshot);
        assert!(
            origin_incoming
                .iter()
                .any(|content| content.contains(REPLY)),
            "reply reached the origin's canonical notification history"
        );
        assert!(
            origin_incoming
                .iter()
                .all(|content| !content.contains("T778_MUST_NOT_ACCEPT_FOREIGN_NOTIFY"))
        );
        assert!(
            !history.contains("PRIVATE_MUST_NOT_READ"),
            "denied private Read leaked no file bytes"
        );
        for kind in ["assignee", "resource"] {
            assert!(
                api.store
                    .get_ticket_assignment_operation(
                        TEST_WORKSPACE_ID,
                        &format!("public-denied-{kind}-{i}")
                    )
                    .unwrap()
                    .is_none(),
                "public refused authority must not reserve Ticket operation"
            );
        }
        let failed = api
            .store
            .get_ticket_assignment_operation(
                TEST_WORKSPACE_ID,
                &format!("public-denied-target-{i}"),
            )
            .unwrap()
            .expect("target validation follows durable start admission");
        assert_eq!(failed.claim_state, "failed");
        assert!(failed.assignment_id.is_none());
        assert!(
            failed.worker.is_none(),
            "target refusal precedes Runtime allocation"
        );
        children.push(child);
    }
    assert_ne!(children[0], children[1]);
    assert_eq!(
        fs::read_to_string(harness._dir.path().join("README.md")).unwrap(),
        "clean\n"
    );
    // Complementary final-dispatch checks use the SAME production connection,
    // after public end-to-end success. They are not sole public-tool evidence.
    let alpha = &cases[0];
    let child = &children[0];
    let forward = api
        .store
        .get_active_worker_control_grant(TEST_WORKSPACE_ID, &alpha.origin, child)
        .unwrap()
        .unwrap();
    api.store
        .revoke_worker_control_grant(
            TEST_WORKSPACE_ID,
            &forward.grant_id,
            &now_registry_timestamp(),
        )
        .unwrap();
    let (status, body) = connected_authorization_post(
        &harness.backend_url,
        &harness.identity,
        &alpha.origin,
        &format!(
            "/api/w/{TEST_WORKSPACE_ID}/worker-control/workers/{}/{}/input",
            child.runtime_id, child.worker_id
        ),
        &json!({"kind":"notify","content":"T778_MUST_NOT_ACCEPT_REVOKED_NOTIFY"}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "revoked directional control: {body}"
    );
    let worker = worker_runtime::identity::WorkerRef::try_from(child).unwrap();
    assert!(
        !serde_json::to_string(
            &harness
                .factory
                .retained_session_snapshot(&worker)
                .unwrap()
                .snapshot
        )
        .unwrap()
        .contains("T778_MUST_NOT_ACCEPT_REVOKED_NOTIFY")
    );
    let previous = subjects
        .subject_permissions(&alpha.subject)
        .unwrap()
        .unwrap();
    let (status, body) = harness.human(reqwest::Method::POST,
        &format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{}/permissions",alpha.subject),
        json!({"operation_id":"public-alpha-revoke-delegation","expected_change_id":previous.change_id,"execution":alpha.execution,"delegation":[]})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mut replay = json!({"runtime_id":CONNECTED_RUNTIME,"display_name":alpha.spawn["display_name"],"profile":alpha.spawn["profile"],
        "ticket_assignment":{"ticket_id":alpha.ticket,"operation_id":alpha.spawn["operation_id"],"orchestrator_assignment_id":alpha.assignment},
        "initial_submit":[],"workdir_attachments":[{"alias":"workdir","working_directory_id":alpha.workdir}],"feature_connections":{},
        "control_operation_id":alpha.spawn["operation_id"],"execution_grant":alpha.spawn["execution_grant"]});
    // Backend replay is exact to public serialization (relative_cwd defaults to null).
    replay["workdir_attachments"][0]["relative_cwd"] = Value::Null;
    let (status, body) = connected_authorization_post(
        &harness.backend_url,
        &harness.identity,
        &alpha.origin,
        &format!("/api/w/{TEST_WORKSPACE_ID}/worker-control/workers"),
        &replay,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "saved successful receipt cannot refresh revoked delegation: {body}"
    );
    // Re-enter the real origin Controller after revocation as well: both public
    // Spawn replay and Notify must consult current authority, not cached results.
    let input = json!({"ticket":alpha.ticket,"spawn":alpha.spawn,"child":target(child)});
    let (status, body) = harness
        .human(
            reqwest::Method::POST,
            &format!(
                "/api/w/{TEST_WORKSPACE_ID}/runtimes/{}/workers/{}/input",
                alpha.origin.runtime_id, alpha.origin.worker_id
            ),
            json!({"kind":"notify","content":format!("{REVOCATION}{input}")}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let revoked = tokio::time::timeout(std::time::Duration::from_secs(45), completion.recv())
        .await
        .expect("public current-authority re-evaluation")
        .expect("public revocation evidence");
    assert_eq!(revoked["revocation"], alpha.ticket);
    assert!(
        !serde_json::to_string(
            &harness
                .factory
                .retained_session_snapshot(&worker)
                .unwrap()
                .snapshot
        )
        .unwrap()
        .contains("T778_MUST_NOT_ACCEPT_REVOKED_NOTIFY")
    );
    assert_eq!(harness.runtime.list_workers().unwrap().len(), 4);
    assert!(
        subjects
            .worker_execution_grant(&child.runtime_id, &child.worker_id)
            .unwrap()
            .is_some(),
        "historical execution evidence retained, not current authority"
    );
    let calls = harness.authorizations.lock().unwrap();
    for name in [
        "worker.create",
        "worker.messaging",
        "filesystem.read",
        "filesystem.write",
    ] {
        assert!(
            calls
                .iter()
                .any(|(request, status)| request["operation"] == name && *status == StatusCode::OK),
            "real common authorization {name}: {calls:?}"
        );
    }
    assert!(
        calls
            .iter()
            .filter(|(request, _)| request["operation"] == "worker.messaging")
            .all(|(request, _)| request["resource"]["kind"] == "worker_operation")
    );
}
