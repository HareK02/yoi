//! Ordinary Subject create -> canonical configuration -> body start -> public
//! Ticket/Coder tools -> real Workdir edit/command/result -> current body tracking.
//! Signed Backend/Runtime, builtin Profiles, Controller, policy and stores are real;
//! only model decisions are scripted. This no-MR scenario submits no Flow;
//! it is not evidence of live-model behavior or of MR review/integration.
use super::*;
#[path = "subject_ticket_conversation_policy_tests.rs"]
mod conversation_policy;
use std::time::Duration;

const NORMAL: &str = "ordinary-subject-work:";
const EDITOR: &str = "ordinary-subject-editor:";
const RECOVER: &str = "ordinary-subject-recover:";
const LIMIT: Duration = Duration::from_secs(60);

#[derive(Clone)]
struct NormalModel {
    edited: tokio::sync::mpsc::Sender<Value>,
    evaluated: tokio::sync::mpsc::Sender<Value>,
    overlap: Arc<tokio::sync::Barrier>,
    seen: Arc<Mutex<std::collections::BTreeSet<String>>>,
}
fn marker(request: &agen::llm_client::Request, prefix: &str) -> Option<Value> {
    request
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Message { content, .. } => Some(content),
            _ => None,
        })
        .flatten()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text),
            _ => None,
        })
        .find_map(|text| {
            text.match_indices(prefix).find_map(|(offset, _)| {
                // An editor marker may be escaped inside the body's JSON spawn input;
                // only the actual external-model message has a raw JSON value here.
                serde_json::Deserializer::from_str(&text[offset + prefix.len()..])
                    .into_iter::<Value>()
                    .next()
                    .and_then(|value| value.ok())
            })
        })
}
fn messages(request: &agen::llm_client::Request) -> String {
    request
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Message { content, .. } => Some(serde_json::to_string(content).unwrap()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
fn tracking_row(value: &Value) -> &Value {
    value
        .as_array()
        .expect("tracking rows")
        .first()
        .expect("current assignment")
}
fn handling(ticket: &str, row: &Value, key: &str, result: &str) -> Value {
    json!({"ticket":ticket,"assignment_id":row["assignment_id"],
        "expected_last_event_id":row["last_event_id"],"operation_key":key,
        "tracking":{"status":"done","waiting_reason":null,"progress":null,
            "next_action":null,"result":result}})
}
// Creation is deliberately idle and Ticket-free. Observe the exact live
// connections through the public Tool, confirm the claim, then submit typed input.
fn assign_and_submit(
    request: &agen::llm_client::Request,
    prefix: &str,
    ticket: &str,
    row: &Value,
    spawned: &Value,
    spawn: &Value,
    segments: Value,
) -> Option<Vec<Event>> {
    assert_eq!(spawn["initial_submit"], json!([]));
    for removed in [
        "ticket_id",
        "orchestrator_assignment_id",
        "initial_request",
        "flow",
        "working_directory_id",
    ] {
        assert!(
            spawn.get(removed).is_none(),
            "general spawn must not carry {removed}"
        );
    }
    let list_id = format!("{prefix}-connections");
    let Some(listed) = success(request, &list_id) else {
        return Some(call(request, &list_id, "WorkerList", json!({})));
    };
    assert_eq!(
        listed["truncated"], false,
        "all known connections must be observable"
    );
    let workers: Vec<_> = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| {
            item["target"]["kind"] == "runtime_worker"
                && item["target"]["runtime_id"] == spawned["runtime_id"]
                && item["target"]["worker_id"] == spawned["worker_id"]
        })
        .collect();
    assert_eq!(
        workers.len(),
        1,
        "use the one Worker just created, not a replacement"
    );
    let worker = workers[0];
    assert!(worker.get("workdir_connections").is_none());
    assert!(worker["worker"].as_str().unwrap().starts_with("W-"));
    let details_id = format!("{prefix}-details");
    let Some(details) = success(request, &details_id) else {
        return Some(call(
            request,
            &details_id,
            "WorkerDetails",
            json!({"target":worker["target"]}),
        ));
    };
    assert_eq!(details["target"], worker["target"]);
    let connections = details["workdir_connections"].as_array().unwrap();
    let attachments = spawn["workdir_attachments"].as_array().unwrap();
    assert_eq!(connections.len(), attachments.len());
    let bindings: Vec<_> = connections.iter().map(|connection| {
        assert!(attachments.iter().any(|attachment| {
            attachment["alias"] == connection["alias"]
                && attachment["working_directory_id"] == connection["working_directory_id"]
        }), "only the selected Workdir attachments may be claimed: {connection}");
        assert!(!connection["connection_id"].as_str().unwrap().is_empty());
        json!({"alias":connection["alias"],"working_directory_id":connection["working_directory_id"],
            "connection_id":connection["connection_id"]})
    }).collect();
    let claim_id = format!("{prefix}-claim");
    let Some(claimed) = success(request, &claim_id) else {
        return Some(call(
            request,
            &claim_id,
            "TicketWorkerAssign",
            json!({
            "ticket":ticket,"runtime_id":spawned["runtime_id"],"worker_id":spawned["worker_id"],
            "orchestrator_assignment_id":row["assignment_id"],
            "operation_id":format!("{prefix}-claim-{ticket}"),"workdir_bindings":bindings}),
        ));
    };
    assert_eq!(claimed["ticket_id"], ticket);
    assert_eq!(claimed["assignment"]["role"], "worker");
    assert!(claimed["assignment"]["assignment_id"].is_string());
    assert_eq!(
        claimed["assignment"]["principal"],
        json!({"kind":"worker",
        "runtime_id":spawned["runtime_id"],"worker_id":spawned["worker_id"]})
    );
    let submit_id = format!("{prefix}-submit");
    let Some(accepted) = success(request, &submit_id) else {
        return Some(call(
            request,
            &submit_id,
            "WorkerSendInput",
            json!({
            "target":worker["target"],"content":"Implement the confirmed Ticket assignment",
            "segments":segments}),
        ));
    };
    // Input acceptance is distinct from the saved result inspected by the caller.
    assert_eq!(accepted["state"], "accepted", "{accepted}");
    None
}
impl NormalModel {
    async fn body(
        &self,
        request: &agen::llm_client::Request,
        input: Value,
        recover: bool,
    ) -> Vec<Event> {
        for forbidden in ["Read", "Write", "Bash", "SubWorkerSpawn"] {
            assert!(!request.tools.iter().any(|tool| tool.name == forbidden));
        }
        let ticket = input["ticket"].as_str().unwrap();
        let tag = if recover { "recover" } else { "normal" };
        let read_id = format!("{tag}-requirements");
        let Some(requirements) = success(request, &read_id) else {
            return call(request, &read_id, "ShowTicket", json!({"id":ticket}));
        };
        assert!(requirements["content_digest"].is_string());
        assert!(
            requirements["initial_post"]["body"]
                .as_str()
                .unwrap()
                .contains("normal requirement")
        );
        assert!(requirements["relations"].is_object());
        if let Some(related) = input["related"].as_str() {
            assert!(
                requirements["relations"]["outgoing"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|relation| relation["ticket"] == related && relation["kind"] == "related")
            );
        }
        assert!(
            requirements["thread"]
                .as_array()
                .unwrap()
                .iter()
                .any(|post| post["body"] == "normal discussion")
        );
        let tracking_id = format!("{tag}-assignment");
        let Some(tracking) = success(request, &tracking_id) else {
            return call(
                request,
                &tracking_id,
                "TicketAssignmentTrackingQuery",
                json!({"ticket":ticket}),
            );
        };
        let row = tracking_row(&tracking);
        assert_eq!(row["principal"]["subject_id"], input["subject"]);
        let result_id = if recover {
            input["result_id"].as_str().unwrap().to_owned()
        } else {
            // Publication and policy are separate: before owner configuration,
            // this same public Tool must fail despite being visible.
            if input["deny"].as_bool().unwrap_or(false) {
                if !refused(request, "normal-no-delegation") {
                    return call(
                        request,
                        "normal-no-delegation",
                        "WorkerSpawn",
                        input["spawn"].clone(),
                    );
                }
                let denial = tool_result(request, "normal-no-delegation").unwrap().1;
                let failure: Value = serde_json::from_str(&denial).unwrap();
                assert_eq!(
                    failure["failure"]["category"], "authorization",
                    "visible WorkerSpawn must reach authorization, not input rejection: {denial}"
                );
                if self.seen.lock().unwrap().insert(format!("denied:{ticket}")) {
                    self.evaluated.send(json!({"denied":ticket})).await.unwrap();
                }
                return done();
            }
            let Some(spawned) = success(request, "normal-spawn") else {
                return call(
                    request,
                    "normal-spawn",
                    "WorkerSpawn",
                    input["spawn"].clone(),
                );
            };
            if let Some(events) = assign_and_submit(
                request,
                "normal",
                ticket,
                row,
                &spawned,
                &input["spawn"],
                json!([{"kind":"text","content":format!("{EDITOR}{}",json!({"ticket":ticket,"content":input["content"]}))}]),
            ) {
                return events;
            }
            // The ordinary business notification contains the stable saved ID.
            // No direct WorkerNotify or shared model side channel supplies it.
            let text = messages(request);
            let Some(offset) = text.find("implementation-result:") else {
                return done();
            };
            text[offset + "implementation-result:".len()..]
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
                .collect::<String>()
        };
        let query_id = format!("{tag}-evaluate");
        let Some(saved) = success(request, &query_id) else {
            return call(
                request,
                &query_id,
                "TicketImplementationResultQuery",
                json!({"ticket":ticket,"result_id":result_id}),
            );
        };
        assert_eq!(saved["ticket_id"], ticket);
        assert_eq!(saved["request"]["body"], input["content"]);
        if !recover {
            let spawned = success(request, "normal-spawn").unwrap();
            let subject = json!({"kind":"runtime_worker","runtime_id":spawned["runtime_id"],"worker_id":spawned["worker_id"]});
            let Some(evidence) = success(request, "normal-evidence-search") else {
                return call(
                    request,
                    "normal-evidence-search",
                    "SearchSessionEntries",
                    json!({"target":subject,"query":"ordinary Subject edit","tool_part":"output","limit":4}),
                );
            };
            let entries = evidence["entries"].as_array().unwrap();
            assert_eq!(entries.len(), 1, "one concrete validation command");
            let Some(inspected) = success(request, "normal-evidence-read") else {
                return call(
                    request,
                    "normal-evidence-read",
                    "ReadSessionEntry",
                    json!({"target":subject,"entry_ref":entries[0]["entry_ref"],"mode":"full"}),
                );
            };
            assert_eq!(inspected["truncated"], false);
            let text = inspected["entries"][0]["text"].as_str().unwrap();
            assert!(
                text.contains("ordinary Subject edit") && text.contains("exit_code"),
                "{text}"
            );
        }
        let update_id = format!("{tag}-tracking");
        if success(request, &update_id).is_none() {
            return call(
                request,
                &update_id,
                "TicketAssignmentTrackingUpdate",
                handling(
                    ticket,
                    row,
                    &format!("{tag}-evaluated-{ticket}"),
                    &result_id,
                ),
            );
        }
        if self.seen.lock().unwrap().insert(format!("{tag}:{ticket}")) {
            self.evaluated
                .send(json!({"ticket":ticket,"result_id":result_id,"recovered":recover}))
                .await
                .unwrap();
        }
        done()
    }
    async fn editor(&self, request: &agen::llm_client::Request, input: Value) -> Vec<Event> {
        let ticket = input["ticket"].as_str().unwrap();
        let Some(requirements) = success(request, "normal-editor-requirements") else {
            return call(
                request,
                "normal-editor-requirements",
                "ShowTicket",
                json!({"id":ticket}),
            );
        };
        assert!(
            requirements["initial_post"]["body"]
                .as_str()
                .unwrap()
                .contains("normal requirement")
        );
        if let Some(receipt) = success(request, "normal-editor-result") {
            if self.seen.lock().unwrap().insert(format!("editor:{ticket}")) {
                self.edited
                    .send(json!({"ticket":ticket,"receipt":receipt}))
                    .await
                    .unwrap();
            }
            return done();
        }
        if let Some((error, output)) = tool_result(request, "normal-editor-check") {
            assert!(!error, "real validation: {output}");
            let validation: Value = serde_json::from_str(&output).unwrap();
            assert_eq!(
                validation["output"]["terminal"]["exit_code"], 0,
                "{validation}"
            );
            assert_eq!(
                validation["output"]["terminal"]["status"], "completed",
                "{validation}"
            );
            return call(
                request,
                "normal-editor-result",
                "TicketImplementationResult",
                json!({"ticket":ticket,"kind":"result","body":input["content"]}),
            );
        }
        if tool_result(request, "normal-editor-write").is_some() {
            assert!(!tool_result(request, "normal-editor-write").unwrap().0);
            return call(
                request,
                "normal-editor-check",
                "Bash",
                json!({"target_workdir":"checkout","cwd":".",
                "command":"test -s README.md && git diff --check && git add README.md && git -c user.name=Test -c user.email=test@example.invalid commit -m 'feat: ordinary Subject edit'","timeout":10000}),
            );
        }
        if let Some((error, text)) = tool_result(request, "normal-editor-read") {
            assert!(!error && text.contains("clean"), "{text}");
            tokio::time::timeout(LIMIT, self.overlap.wait())
                .await
                .expect("two independent builtin Coders overlap");
            return call(
                request,
                "normal-editor-write",
                "Write",
                json!({"target_workdir":"checkout","file_path":"README.md","content":input["content"]}),
            );
        }
        call(
            request,
            "normal-editor-read",
            "Read",
            json!({"target_workdir":"checkout","file_path":"README.md"}),
        )
    }
}
#[async_trait::async_trait]
impl agen::llm_client::LlmClient for NormalModel {
    fn clone_boxed(&self) -> Box<dyn agen::llm_client::LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        request: agen::llm_client::Request,
    ) -> std::result::Result<agen::llm_client::client::ResponseStream, agen::llm_client::ClientError>
    {
        let events = if let Some(input) = marker(&request, EDITOR) {
            self.editor(&request, input).await
        } else if let Some(input) = marker(&request, RECOVER) {
            self.body(&request, input, true).await
        } else if let Some(input) = marker(&request, NORMAL) {
            self.body(&request, input, false).await
        } else {
            done()
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}
async fn ok(h: &ConnectedWorkerHarness, method: &str, path: &str, input: Value) -> Value {
    let (status, body) = h.human(method.parse().unwrap(), path, input).await;
    assert_eq!(status, StatusCode::OK, "{method} {path}: {body}");
    body
}
fn body_path(subject: &str) -> String {
    format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{subject}/body")
}
async fn start(h: &ConnectedWorkerHarness, subject: &str, key: &str) -> Value {
    let response = ok(
        h,
        "POST",
        &format!("{}/start", body_path(subject)),
        json!({"request_id":key,"expected_current":null,"expected_observation_token":null}),
    )
    .await;
    assert_eq!(response["disposition"], "accepted", "{response}");
    response["body"].clone()
}
async fn send(
    h: &ConnectedWorkerHarness,
    conversation: &str,
    key: &str,
    prefix: &str,
    input: &Value,
) {
    let result = ok(
        h,
        "POST",
        &format!("/api/w/{TEST_WORKSPACE_ID}/chat/conversations/{conversation}/messages"),
        json!({"operation_id":key,"text":format!("{prefix}{input}")}),
    )
    .await;
    assert_eq!(result["delivery"]["state"], "accepted", "{result}");
}
fn persisted_tracking(h: &ConnectedWorkerHarness, ticket: &str) -> Value {
    serde_json::to_value(
        h.api
            .store
            .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, ticket)
            .unwrap(),
    )
    .unwrap()
}
async fn next(rx: &mut tokio::sync::mpsc::Receiver<Value>) -> Value {
    tokio::time::timeout(LIMIT, rx.recv())
        .await
        .expect("ordinary Subject model path reached")
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn ordinary_two_subjects_builtin_coders_edit_validate_save_evaluate_and_track() {
    crate::restore_test_environment::require_restore_test_environment();
    let (edited, mut edits) = tokio::sync::mpsc::channel(4);
    let (evaluated, mut evaluations) = tokio::sync::mpsc::channel(8);
    let h = ConnectedWorkerHarness::start(NormalModel {
        edited,
        evaluated,
        overlap: Arc::new(tokio::sync::Barrier::new(2)),
        seen: Default::default(),
    })
    .await;
    h.api
        .runtime
        .unregister_if_idle(EMBEDDED_WORKER_RUNTIME_ID, 100)
        .unwrap();
    let subjects = open_subjektiv_store(&h.api).unwrap();
    let mut cases = vec![];
    for label in ["alpha", "beta"] {
        let path = format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects");
        let (status, subject) = h
            .human(
                reqwest::Method::POST,
                &path,
                json!({"operation_id":format!("ordinary-{label}"),"role":label,"behavior_md":""}),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{subject}");
        let subject = subject["id"].as_str().unwrap().to_owned();
        let created_policy = subjects.current_subject_policy(&subject).unwrap().unwrap();
        assert_eq!(
            created_policy.execution,
            server_api::authorization::subject_default_permission_assignment()
        );
        assert_eq!(
            created_policy.delegation,
            server_api::authorization::subject_default_delegation_assignment()
        );
        let branch = format!("ordinary-{label}");
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(h._dir.path())
                .args(["branch", &branch])
                .status()
                .unwrap()
                .success()
        );
        let (status, wd) = h.human(reqwest::Method::POST, &format!("/api/w/{TEST_WORKSPACE_ID}/working-directories"),
            json!({"runtime_id":CONNECTED_RUNTIME,"repository_key":"test-repository","selector":branch,"operation_id":format!("ordinary-wd-{label}")})).await;
        assert_eq!(status, StatusCode::CREATED, "{wd}");
        let wd = wd["item"]["working_directory_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let root = h
            .materializer
            .bind_working_directory(&wd, None)
            .unwrap()
            .root()
            .to_owned();
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["switch", "-c", &branch])
                .status()
                .unwrap()
                .success()
        );
        // Carry the actual creation assignments through canonical enrollment.
        // Tailored child handling authority is tested separately at the signed
        // router boundary, without replaying this entire connected scenario.
        super::super::chat_tests::adopt_chat_defaults(&h.api, &h.owner).await;
        assert_eq!(
            subjects.current_subject_policy(&subject).unwrap().unwrap(),
            created_policy
        );
        let body = start(&h, &subject, &format!("ordinary-start-{label}")).await;
        assert_eq!(
            body["observation"]["worker"]["profile"],
            "builtin:async-chat"
        );
        let mut ticket = NewTicket::new(format!("ordinary {label}"));
        ticket.body = "normal requirement".into();
        set_test_ticket_target(&mut ticket, "test-repository", &branch);
        let ticket = browser_ticket_backend(&h.api)
            .unwrap()
            .create(ticket)
            .unwrap()
            .id;
        browser_ticket_backend(&h.api)
            .unwrap()
            .add_event(
                ticket.clone().into(),
                ticket::NewTicketEvent::new(ticket::TicketEventKind::Comment, "normal discussion"),
            )
            .unwrap();
        let related = browser_ticket_backend(&h.api)
            .unwrap()
            .create(NewTicket::new("Relevant context, not delegated work"))
            .unwrap()
            .id;
        browser_ticket_backend(&h.api)
            .unwrap()
            .add_ticket_relation(
                ticket.clone().into(),
                ticket::NewTicketRelation {
                    kind: ticket::TicketRelationKind::Related,
                    target: related.clone(),
                    note: Some("normal relation".into()),
                    author: None,
                },
            )
            .unwrap();
        let related = browser_ticket_backend(&h.api)
            .unwrap()
            .show(related.into())
            .unwrap()
            .meta
            .resource_key
            .unwrap();
        let _assigned = ok(&h, "PUT", &format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator"),
            json!({"operation_id":format!("ordinary-assign-{label}"),"expected_assignment_id":null,"principal":{"kind":"subject","subject_id":subject}})).await;
        let conversation = ok(
            &h,
            "POST",
            &format!("/api/w/{TEST_WORKSPACE_ID}/chat/conversations"),
            json!({"operation_id":format!("ordinary-chat-{label}"),"subject_id":subject}),
        )
        .await;
        let content = format!("normal {label} verified\n");
        // Operation authority is separate from the selected Ticket and Workdir.
        // The Host freezes the assignment within the current Subject delegation.
        let assignment = standalone_assignment(
            r#"{ namespaces = [
                { namespace = "filesystem"; keys = ["filesystem.read", "filesystem.write"]; },
                { namespace = "bash"; keys = ["bash.execute"]; },
                { namespace = "ticket"; keys = ["ticket.read", "ticket.relation.read", "ticket.implementation_result.write"]; }
            ]; }"#,
        );
        let spawn = json!({"runtime_id":CONNECTED_RUNTIME,"profile":"builtin:coder",
            "operation_id":format!("ordinary-spawn-{label}"),"initial_submit":[],
            "workdir_attachments":[{"alias":"checkout","working_directory_id":wd}],
            "authorization":{"kind":"assignment","assignment":assignment}});
        let input = json!({"subject":subject,"ticket":ticket,"spawn":spawn,"content":content,"related":related});
        cases.push((
            input,
            conversation["id"].as_str().unwrap().to_owned(),
            root,
            body,
        ));
    }
    for (index, (input, conversation, _, _)) in cases.iter().enumerate() {
        send(
            &h,
            conversation,
            &format!("ordinary-request-{index}"),
            NORMAL,
            input,
        )
        .await;
    }
    let mut results = std::collections::BTreeMap::new();
    for _ in 0..2 {
        let edit = next(&mut edits).await;
        results.insert(
            edit["ticket"].as_str().unwrap().to_owned(),
            edit["receipt"].clone(),
        );
    }
    for _ in 0..2 {
        let evaluation = next(&mut evaluations).await;
        assert_eq!(
            evaluation["result_id"],
            results[evaluation["ticket"].as_str().unwrap()]["result_id"]
        );
    }
    for (input, _, root, body) in &cases {
        let ticket = input["ticket"].as_str().unwrap();
        assert_eq!(
            fs::read_to_string(root.join("README.md")).unwrap(),
            input["content"].as_str().unwrap()
        );
        let clean = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        assert!(clean.status.success() && clean.stdout.is_empty());
        let row = h
            .api
            .store
            .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, ticket)
            .unwrap()
            .remove(0);
        assert_eq!(
            row.tracking.unwrap().status,
            ticket::TicketAssignmentTrackingStatus::Done
        );
        assert_eq!(
            h.api.authority.ticket(ticket).unwrap().state,
            server_api::BrowserTicketState::Open
        );
        // Real saved manifest is the ordinary builtin, not a test-only profile.
        let source = RuntimeWorkerRef::new(
            CONNECTED_RUNTIME,
            body["current"]["worker_id"].as_str().unwrap(),
        );
        let snapshot = h
            .factory
            .retained_session_snapshot(
                &worker_runtime::identity::WorkerRef::try_from(&source).unwrap(),
            )
            .unwrap();
        let incoming = notifications(&snapshot.snapshot);
        assert!(
            incoming
                .iter()
                .any(|text| text.contains("implementation-result:"))
        );
        // The ordinary Coder cannot self-report through the signed endpoint.
        // The separate subject_handling_authority test grants the update key
        // explicitly and proves rejection by current-body ownership, independent
        // of capability denial or the bounded Coder Profile's Tool set.
        let child = h.api.config_store.with_conn(|conn| {
            conn.query_row("SELECT runtime_id,worker_id FROM ticket_implementation_results WHERE workspace_id=?1 AND result_id=?2",
                rusqlite::params![TEST_WORKSPACE_ID,results[ticket]["result_id"].as_str().unwrap()],
                |row| Ok(RuntimeWorkerRef::new(row.get::<_,String>(0)?, row.get::<_,String>(1)?))).map_err(Into::into)
        }).unwrap();
        let grant = h
            .api
            .config_store
            .core_worker_execution_grant(TEST_WORKSPACE_ID, &child.runtime_id, &child.worker_id)
            .unwrap()
            .expect("public spawn persists a current core grant");
        assert_eq!(
            serde_json::to_value(&grant.assignment).unwrap(),
            input["spawn"]["authorization"]["assignment"]
        );
        assert!(grant.issuance_ceiling.is_some());
        assert!(matches!(
            &grant.source,
            server_api::authorization::CoreWorkerGrantSource::Delegation {
                parent: server_api::authorization::CoreWorkerGrantParent::SubjectBody {
                    subject_id, runtime_id, worker_id, ..
                }
            } if subject_id == input["subject"].as_str().unwrap()
                && runtime_id == &source.runtime_id && worker_id == &source.worker_id
        ));
        let tracking = persisted_tracking(&h, ticket);
        let mut denied = handling(
            ticket,
            tracking_row(&tracking),
            "background-must-not-report",
            "not a Subject judgment",
        );
        denied.as_object_mut().unwrap().remove("ticket");
        let (status, error) = connected_authorization_post(
            &h.backend_url,
            &h.identity,
            &child,
            &format!("/workspaces/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignment-tracking"),
            &denied,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "ordinary background self-report denied: {error}"
        );
    }

    // Restore reads the canonical result rather than spawning a duplicate Coder.
    let (input, conversation, _, _) = &cases[0];
    let subject = input["subject"].as_str().unwrap();
    let ticket = input["ticket"].as_str().unwrap();
    let before_policy = subjects.current_subject_policy(subject).unwrap();
    let observed = ok(&h, "GET", &body_path(subject), Value::Null).await;
    let stop = json!({"request_id":"ordinary-stop","expected_current":observed["current"],
        "expected_observation_token":observed["observation"]["worker"]["restore_observation_token"]});
    let stopped = ok(
        &h,
        "POST",
        &format!("{}/stop", body_path(subject)),
        stop.clone(),
    )
    .await;
    assert_eq!(stopped["disposition"], "accepted", "{stopped}");
    let outcome = ok(
        &h,
        "POST",
        &format!("{}/stop/outcome", body_path(subject)),
        stop,
    )
    .await;
    assert_eq!(outcome["outcome"], "stopped", "{outcome}");
    let observed = ok(&h, "GET", &body_path(subject), Value::Null).await;
    let restored = ok(&h,"POST",&format!("{}/start",body_path(subject)),
        json!({"request_id":"ordinary-restore","expected_current":observed["current"],
            "expected_observation_token":observed["observation"]["worker"]["restore_observation_token"]})).await;
    assert_eq!(restored["disposition"], "accepted", "{restored}");
    let mut recovery = input.clone();
    recovery["result_id"] = results[ticket]["result_id"].clone();
    send(
        &h,
        conversation,
        "ordinary-recover-result",
        RECOVER,
        &recovery,
    )
    .await;
    assert_eq!(next(&mut evaluations).await["recovered"], true);
    assert_eq!(
        subjects.current_subject_policy(subject).unwrap(),
        before_policy
    );
    assert_eq!(h.api.config_store.with_conn(|conn|conn.query_row("SELECT count(*) FROM ticket_implementation_results WHERE workspace_id=?1 AND ticket_id=?2",
        rusqlite::params![TEST_WORKSPACE_ID,ticket],|row|row.get::<_,i64>(0)).map_err(Into::into)).unwrap(),1);

    // Reassignment fences the former body even though its operation capability
    // remains unchanged. It does not automatically start/restore any Worker.
    let tracking = persisted_tracking(&h, ticket);
    let old_row = tracking_row(&tracking);
    let reassigned = ok(
        &h,
        "PUT",
        &format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator"),
        json!({"operation_id":"ordinary-reassign","expected_assignment_id":old_row["assignment_id"],
            "principal":{"kind":"subject","subject_id":cases[1].0["subject"]}}),
    )
    .await;
    assert_ne!(
        reassigned["assignment"]["assignment_id"],
        old_row["assignment_id"]
    );
    let mut stale = handling(
        ticket,
        old_row,
        "old-subject-must-not-report",
        "stale result",
    );
    stale.as_object_mut().unwrap().remove("ticket");
    let (status, error) = connected_authorization_post(
        &h.backend_url,
        &h.identity,
        &RuntimeWorkerRef::new(
            CONNECTED_RUNTIME,
            restored["body"]["current"]["worker_id"].as_str().unwrap(),
        ),
        &format!("/workspaces/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignment-tracking"),
        &stale,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "replaced assignment CAS: {error}"
    );
    let current = persisted_tracking(&h, ticket);
    let mut unauthorized = handling(
        ticket,
        tracking_row(&current),
        "former-subject-current-row",
        "not the assignee",
    );
    unauthorized.as_object_mut().unwrap().remove("ticket");
    let (status, error) = connected_authorization_post(
        &h.backend_url,
        &h.identity,
        &RuntimeWorkerRef::new(
            CONNECTED_RUNTIME,
            restored["body"]["current"]["worker_id"].as_str().unwrap(),
        ),
        &format!("/workspaces/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignment-tracking"),
        &unauthorized,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "former Subject with current CAS cannot act: {error}"
    );
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("authenticated Subject does not own this assignment"),
        "{error}"
    );
    assert_eq!(persisted_tracking(&h, ticket), current);
    let next_subject = cases[1].0["subject"].as_str().unwrap();
    super::super::chat_tests::adopt_subject_policy(&h.api,&h.owner,next_subject,
        r#"{ sets = ["conversation", "work-coordination"]; exclude_namespaces = [{ namespace = "ticket"; keys = ["ticket.assignment.update"]; }]; }"#,"{}").await;
    let tracking_path =
        format!("/workspaces/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignment-tracking");
    let before = persisted_tracking(&h, ticket);
    let mut revoked = handling(
        ticket,
        tracking_row(&before),
        "revoked-must-not-report",
        "revoked result",
    );
    revoked.as_object_mut().unwrap().remove("ticket");
    let (status, error) = connected_authorization_post(
        &h.backend_url,
        &h.identity,
        &RuntimeWorkerRef::new(
            CONNECTED_RUNTIME,
            cases[1].3["current"]["worker_id"].as_str().unwrap(),
        ),
        &tracking_path,
        &revoked,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "current body must recheck excluded capability: {error}"
    );
    assert_eq!(persisted_tracking(&h, ticket), before);
    super::super::chat_tests::adopt_subject_policy(&h.api, &h.owner, next_subject, "{}", "{}")
        .await;
    let empty = subjects
        .current_subject_policy(next_subject)
        .unwrap()
        .unwrap();
    assert!(empty.execution.grants.is_empty() && empty.delegation.grants.is_empty());
    let (status, error) = connected_authorization_post(
        &h.backend_url,
        &h.identity,
        &RuntimeWorkerRef::new(
            CONNECTED_RUNTIME,
            cases[1].3["current"]["worker_id"].as_str().unwrap(),
        ),
        &tracking_path,
        &revoked,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "explicit empty does not revive default: {error}"
    );
    assert_eq!(
        subjects
            .current_subject_policy(next_subject)
            .unwrap()
            .unwrap(),
        empty
    );
}
