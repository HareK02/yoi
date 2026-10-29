//! Public Worker tools over the shared signed Backend/Runtime connection. The
//! external model alone is scripted; permissions API, Controller, tool engine,
//! control grants, notification admission and canonical Session storage are real.
//! No developer model credentials or live Runtime state are used.
use super::*;
use agen::llm_client::{
    event::{Event, StopReason},
    types::Item,
};
use worker_runtime::worker_backend::RuntimeWorkerFactory;

const ORIGIN: &str = "ROUNDTRIP_ORIGIN";
const PING: &str = "ROUNDTRIP_QUESTION";
const REPLY: &str = "ROUNDTRIP_PROGRESS_RESULT";
const EARLY: &str = "ROUNDTRIP_INITIAL_CHILD";

fn operation(operation: &str) -> ScopedPermission {
    ScopedPermission {
        operation: operation.into(),
        resource: PermissionResource::WorkerOperation {
            workspace_id: TEST_WORKSPACE_ID.into(),
        },
    }
}
fn collection(operation: &str) -> ScopedPermission {
    ScopedPermission {
        operation: operation.into(),
        resource: PermissionResource::Workspace {
            workspace_id: TEST_WORKSPACE_ID.into(),
        },
    }
}
fn result(request: &agen::llm_client::Request, id: &str) -> Option<Value> {
    request.items.iter().find_map(|item| match item {
        Item::ToolResult {
            call_id,
            is_error,
            content,
            summary,
            ..
        } if call_id == id => {
            assert!(!is_error, "public tool {id}: {summary}; {content:?}");
            Some(serde_json::from_str(content.as_deref().expect("public JSON result")).unwrap())
        }
        _ => None,
    })
}
fn call(id: &str, name: &str, input: Value) -> Vec<Event> {
    vec![
        Event::tool_use_start(0, id, name),
        Event::tool_input_delta(0, input.to_string()),
        Event::tool_use_stop(0),
    ]
}
fn done() -> Vec<Event> {
    vec![
        Event::text_block_start(0),
        Event::text_delta(0, "roundtrip observed"),
        Event::text_block_stop(0, Some(StopReason::EndTurn)),
    ]
}

struct RepairSignals {
    early_observation: tokio::sync::mpsc::Sender<bool>,
    spawn_refusal: tokio::sync::mpsc::Sender<()>,
    allow_replay: tokio::sync::Notify,
    origin: Mutex<Option<RuntimeWorkerRef>>,
    origin_ready: tokio::sync::Notify,
}
#[derive(Clone)]
struct MessagingModel {
    child_permissions: Arc<Mutex<Vec<ScopedPermission>>>,
    workdir: Arc<Mutex<Option<String>>>,
    completed: tokio::sync::mpsc::Sender<Value>,
    child_replied: Arc<tokio::sync::Notify>,
    repair: Option<Arc<RepairSignals>>,
}
#[async_trait::async_trait]
impl agen::llm_client::LlmClient for MessagingModel {
    fn clone_boxed(&self) -> Box<dyn agen::llm_client::LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        request: agen::llm_client::Request,
    ) -> std::result::Result<agen::llm_client::client::ResponseStream, agen::llm_client::ClientError>
    {
        let messages = request
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Message { content, .. } => Some(serde_json::to_string(content).unwrap()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let origin = messages.contains(ORIGIN);
        let events = if origin {
            if let Some(stopped) = result(&request, "roundtrip-stop") {
                assert_eq!(stopped["state"], "accepted", "{stopped}");
                let child = result(&request, "roundtrip-spawn").unwrap();
                if let Some(is_error) = request.items.iter().find_map(|item| match item {
                    Item::ToolResult {
                        call_id, is_error, ..
                    } if call_id == "stopped-notify" => Some(*is_error),
                    _ => None,
                }) {
                    assert!(
                        is_error,
                        "Notify to stopped child must refuse, never restore or claim success"
                    );
                    let _ = self
                        .completed
                        .try_send(json!({"spawn":child,"messages":messages}));
                    done()
                } else {
                    call(
                        "stopped-notify",
                        "WorkerNotify",
                        json!({"subject":{"kind":"runtime_worker","runtime_id":child["runtime_id"],"worker_id":child["worker_id"]},"content":"MUST_NOT_REVIVE_STOPPED_CHILD"}),
                    )
                }
            } else if messages.contains(REPLY) {
                let child = result(&request, "roundtrip-spawn").unwrap();
                call(
                    "roundtrip-stop",
                    "WorkerStop",
                    json!({"subject":{"kind":"runtime_worker","runtime_id":child["runtime_id"],"worker_id":child["worker_id"]},"reason":"connected roundtrip completed"}),
                )
            } else if result(&request, "roundtrip-ping").is_some() {
                // Hold this model boundary while the real child replies to a Busy
                // origin. This is a deterministic external-model barrier, not a
                // replacement for notification admission or Controller scheduling.
                tokio::time::timeout(
                    std::time::Duration::from_secs(30),
                    self.child_replied.notified(),
                )
                .await
                .expect("child public reply result");
                done()
            } else if let Some(child) = result(&request, "roundtrip-spawn") {
                if result(&request, "roundtrip-observe").is_some() {
                    call(
                        "roundtrip-ping",
                        "WorkerNotify",
                        json!({"subject":{"kind":"runtime_worker","runtime_id":child["runtime_id"],"worker_id":child["worker_id"]},"content":PING}),
                    )
                } else {
                    // The origin has observe, not worker.list. A known target must
                    // remain readable without collection discovery authority.
                    call(
                        "roundtrip-observe",
                        "ViewSessionOverview",
                        json!({"subject":{"kind":"runtime_worker","runtime_id":child["runtime_id"],"worker_id":child["worker_id"]}}),
                    )
                }
            } else {
                let call_id = if let Some(repair) = &self.repair {
                    if let Some((is_error, summary)) =
                        request.items.iter().find_map(|item| match item {
                            Item::ToolResult {
                                call_id,
                                is_error,
                                summary,
                                ..
                            } if call_id == "roundtrip-spawn-first" => Some((*is_error, summary)),
                            _ => None,
                        })
                    {
                        assert!(
                            is_error,
                            "grant-save fault must not report successful Spawn: {summary}"
                        );
                        let _ = repair.spawn_refusal.try_send(());
                        tokio::time::timeout(
                            std::time::Duration::from_secs(30),
                            repair.allow_replay.notified(),
                        )
                        .await
                        .expect("explicit original-operation replay authorization");
                        "roundtrip-spawn"
                    } else {
                        "roundtrip-spawn-first"
                    }
                } else {
                    "roundtrip-spawn"
                };
                let mut input = json!({"runtime_id":CONNECTED_RUNTIME,"profile":"project:messaging-peer","operation_id":"public-roundtrip-spawn","display_name":"Notify-only delegated peer","initial_submit":if self.repair.is_some() {vec![protocol::Segment::Text {content:EARLY.into()}]} else {vec![]},"execution_grant":{"permissions":self.child_permissions.lock().unwrap().clone(),"redelegation":[]}});
                if let Some(workdir) = self.workdir.lock().unwrap().as_ref() {
                    input["working_directory_id"] = json!(workdir);
                }
                call(call_id, "WorkerSpawn", input)
            }
        } else if self.repair.is_some() && !messages.contains(PING) {
            assert!(
                messages.contains(EARLY),
                "early child started through original initial Submit"
            );
            if let Some(available) = request.items.iter().find_map(|item| match item {
                Item::ToolResult {
                    call_id,
                    is_error,
                    content,
                    ..
                } if call_id == "early-origin-list" => {
                    let available = !is_error
                        && content.as_deref().is_some_and(|text| {
                            serde_json::from_str::<Value>(text).unwrap()["items"]
                                .as_array()
                                .is_some_and(|items| !items.is_empty())
                        });
                    Some(available)
                }
                _ => None,
            }) {
                assert!(
                    !available,
                    "an unpersisted reply grant cannot be discovered as authority"
                );
                let repair = self.repair.as_ref().unwrap();
                if let Some(is_error) = request.items.iter().find_map(|item| match item {
                    Item::ToolResult {
                        call_id, is_error, ..
                    } if call_id == "early-denied-notify" => Some(*is_error),
                    _ => None,
                }) {
                    assert!(
                        is_error,
                        "missing reply grant must deny public Notify, not report acceptance"
                    );
                    let _ = repair.early_observation.try_send(available);
                    done()
                } else {
                    tokio::time::timeout(
                        std::time::Duration::from_secs(30),
                        repair.origin_ready.notified(),
                    )
                    .await
                    .expect("test obtains original origin identity");
                    // The harness supplies only a target identity, never authority.
                    // Discovery was empty; even knowing the ID cannot authorize Notify.
                    let origin = repair.origin.lock().unwrap().clone().unwrap();
                    call(
                        "early-denied-notify",
                        "WorkerNotify",
                        json!({"subject":{"kind":"runtime_worker","runtime_id":origin.runtime_id,"worker_id":origin.worker_id},"content":"MUST_NOT_ACCEPT_EARLY_REPLY"}),
                    )
                }
            } else {
                call("early-origin-list", "WorkerList", json!({}))
            }
        } else {
            assert!(
                messages.contains(PING),
                "child starts from real Idle Notify, not hidden initial Submit: {messages}"
            );
            if let Some(reply) = result(&request, "roundtrip-reply") {
                assert_eq!(reply["state"], "accepted", "{reply}");
                self.child_replied.notify_one();
                done()
            } else if let Some(list) = result(&request, "roundtrip-origin-list") {
                let origins = list["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|item| item["relation"] == "reply")
                    .collect::<Vec<_>>();
                assert_eq!(
                    origins.len(),
                    1,
                    "public discovery must expose only the confirmed reply origin: {list}"
                );
                assert_eq!(origins[0]["permissions"], json!(["notify"]));
                call(
                    "roundtrip-reply",
                    "WorkerNotify",
                    json!({"subject":origins[0]["subject"],"content":REPLY}),
                )
            } else {
                call("roundtrip-origin-list", "WorkerList", json!({}))
            }
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}

fn install_messaging_profiles(api: &WorkspaceApi) {
    let state = api
        .config_store
        .load_workspace_config(TEST_WORKSPACE_ID)
        .unwrap()
        .unwrap();
    let main = state
        .snapshot
        .get(&config_source::VirtualPath::parse("main.dcdl").unwrap())
        .unwrap();
    let content = r#"{ profile = { default_profile = "project:messaging-origin"; entries = [
        { selector = "project:messaging-origin"; label = "Messaging origin"; description = "Public delegation boundary";
          profile = (import "$builtin/profiles/companion.dcdl") // { slug = "messaging-origin"; scope = "workspace_read"; delegation_scope = "workspace_read"; feature = { worker = { enabled = true; direct_spawn = true; }; }; };
        },
        { selector = "project:messaging-peer"; label = "Messaging peer"; description = "Notify-only delegated peer";
          profile = (import "$builtin/profiles/companion.dcdl") // { slug = "messaging-peer"; scope = "workspace_read"; feature = { subjektiv = { enabled = false; }; worker = { enabled = true; direct_spawn = false; }; }; };
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

async fn connected_roundtrip(explicit_workdir: bool, repair: bool) {
    use worker_runtime::working_directory::WorkingDirectoryMaterializer;
    let (completed, mut completion) = tokio::sync::mpsc::channel(2);
    let permissions = Arc::new(Mutex::new(vec![
        operation("worker.messaging"),
        collection("worker.list"),
    ]));
    let workdir = Arc::new(Mutex::new(None));
    let (early_observation, mut early) = tokio::sync::mpsc::channel(1);
    let (spawn_refusal, mut refused) = tokio::sync::mpsc::channel(1);
    let repair_signals = repair.then(|| {
        Arc::new(RepairSignals {
            early_observation,
            spawn_refusal,
            allow_replay: tokio::sync::Notify::new(),
            origin: Mutex::new(None),
            origin_ready: tokio::sync::Notify::new(),
        })
    });
    let harness = ConnectedWorkerHarness::start(MessagingModel {
        child_permissions: permissions.clone(),
        workdir: workdir.clone(),
        completed,
        child_replied: Arc::new(tokio::sync::Notify::new()),
        repair: repair_signals.clone(),
    })
    .await;
    let api = &harness.api;
    install_messaging_profiles(api);
    if explicit_workdir {
        let (status, created) = response(api, human_request(&harness.owner, "POST", &format!("/api/w/{TEST_WORKSPACE_ID}/working-directories"), Some(json!({"runtime_id":CONNECTED_RUNTIME,"repository_key":"test-repository","selector":"HEAD","operation_id":"public-messaging-workdir"})))).await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let id = created["item"]["working_directory_id"]
            .as_str()
            .unwrap()
            .to_owned();
        permissions.lock().unwrap().push(ScopedPermission {
            operation: "filesystem.read".into(),
            resource: PermissionResource::Workdir {
                working_directory_id: id.clone(),
                path: ".".into(),
                recursive: true,
            },
        });
        *workdir.lock().unwrap() = Some(id);
    }
    let store = open_subjektiv_store(api).unwrap();
    let subject = store
        .create_subject(crate::subjektiv::SubjectRole::new("public-messaging").unwrap())
        .unwrap()
        .id;
    let mut execution = [
        "session.record",
        "memory.resident.read",
        "memory.resident.supply",
        "journal.startup.read",
        "journal.startup.supply",
    ]
    .into_iter()
    .map(|op| subject_permission(&subject, op))
    .collect::<Vec<_>>();
    execution.extend([
        collection("worker.create"),
        operation("worker.messaging"),
        operation("worker.stop"),
        operation("worker.observe"),
    ]);
    let (status, assigned) = response(api, human_request(&harness.owner,"POST",&format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{subject}/permissions"),Some(json!({"operation_id":"public-messaging-policy","expected_change_id":null,"execution":execution,"delegation":permissions.lock().unwrap().clone()})))).await;
    assert_eq!(status, StatusCode::OK, "{assigned}");
    let before = store.subject_permissions(&subject).unwrap().unwrap();
    if repair {
        api.config_store.with_conn(|connection| {
            connection.execute_batch("CREATE TRIGGER t803_reply_save_fault BEFORE INSERT ON worker_control_grants WHEN NEW.relation='reply' BEGIN SELECT RAISE(ABORT,'injected reply grant save failure'); END;")?;
            Ok(())
        }).unwrap();
    }
    let Json(source) = create_workspace_worker(
        State(api.clone()),
        HeaderMap::new(),
        Json(CreateWorkspaceWorkerRequest {
            runtime_id: CONNECTED_RUNTIME.into(),
            display_name: "Public WorkerSpawn Subject origin".into(),
            singleton_key: None,
            profile: Some("project:messaging-origin".into()),
            ticket_assignment: None,
            initial_submit: vec![protocol::Segment::Text {
                content: ORIGIN.into(),
            }],
            workdir_attachments: vec![],
            feature_connections: server_api::WorkspaceWorkerFeatureConnectionsRequest {
                subjektiv: Some(server_api::SubjektivWorkerConnectionRequest {
                    subject_id: subject.clone(),
                }),
            },
            control_operation_id: None,
            execution_grant: None,
        }),
    )
    .await
    .unwrap_or_else(|error| {
        panic!(
            "origin: {error:?}; Runtime: {:?}",
            harness.runtime_errors.lock().unwrap()
        )
    });
    if let Some(signals) = &repair_signals {
        *signals.origin.lock().unwrap() =
            Some(RuntimeWorkerRef::new(&source.runtime_id, &source.worker_id));
        signals.origin_ready.notify_one();
        assert!(
            !tokio::time::timeout(std::time::Duration::from_secs(60), early.recv())
                .await
                .expect("earliest child public discovery")
                .unwrap()
        );
        tokio::time::timeout(std::time::Duration::from_secs(60), refused.recv())
            .await
            .expect("public Spawn save-failure refusal")
            .unwrap();
        assert_eq!(
            harness.runtime.list_workers().unwrap().len(),
            2,
            "confirmed original child is retained despite reply-save failure"
        );
        api.config_store
            .with_conn(|connection| {
                connection.execute_batch("DROP TRIGGER t803_reply_save_fault")?;
                Ok(())
            })
            .unwrap();
        signals.allow_replay.notify_one();
    }
    let observed = tokio::time::timeout(std::time::Duration::from_secs(60), completion.recv())
        .await
        .expect("public Spawn/Notify/reply/Stop complete")
        .expect("model completion evidence");
    let child = RuntimeWorkerRef::new(
        observed["spawn"]["runtime_id"].as_str().unwrap(),
        observed["spawn"]["worker_id"].as_str().unwrap(),
    );
    let origin = RuntimeWorkerRef::new(source.runtime_id, source.worker_id);
    assert!(observed["messages"].as_str().unwrap().contains(REPLY));
    assert_eq!(
        store.subject_permissions(&subject).unwrap().unwrap(),
        before,
        "allocated ID was never appended to Subject policy"
    );
    let forward = api
        .store
        .get_active_worker_control_grant(TEST_WORKSPACE_ID, &origin, &child)
        .unwrap()
        .unwrap();
    assert!(forward.permissions.contains(&"stop".into()));
    let reverse = api
        .store
        .get_active_worker_control_grant(TEST_WORKSPACE_ID, &child, &origin)
        .unwrap()
        .unwrap();
    assert_eq!(reverse.permissions, vec!["notify".to_owned()]);
    let child_ref = worker_runtime::identity::WorkerRef::try_from(&child).unwrap();
    let snapshot = harness
        .factory
        .retained_session_snapshot(&child_ref)
        .unwrap();
    let history = serde_json::to_string(&snapshot.snapshot).unwrap();
    assert!(
        !history.contains("MUST_NOT_REVIVE_STOPPED_CHILD"),
        "stopped destination received no notification/mailbox input"
    );
    assert_eq!(
        harness.runtime.worker_detail(&child_ref).unwrap().status,
        worker_runtime::catalog::WorkerStatus::Stopped
    );
    if repair {
        assert_eq!(
            snapshot
                .snapshot
                .entries
                .iter()
                .filter(|item| serde_json::to_string(item).unwrap().contains(EARLY))
                .count(),
            1,
            "original initial input was not redispatched on recovery"
        );
        assert_eq!(
            harness.runtime.list_workers().unwrap().len(),
            2,
            "one origin and one child after original-operation replay"
        );
    }
    assert!(
        history.contains(PING),
        "Idle notification persisted before model supply"
    );
    let source_ref = worker_runtime::identity::WorkerRef::try_from(&origin).unwrap();
    let origin_history = serde_json::to_string(
        &harness
            .factory
            .retained_session_snapshot(&source_ref)
            .unwrap()
            .snapshot,
    )
    .unwrap();
    assert!(
        !origin_history.contains("MUST_NOT_ACCEPT_EARLY_REPLY"),
        "unauthorized early Notify caused no origin input"
    );
    assert!(
        origin_history.contains(REPLY),
        "Busy reply persisted before origin model supply"
    );
    assert!(
        api.store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, &origin)
            .unwrap()
            .is_empty()
    );
    let links = api
        .store
        .list_worker_workdir_links(TEST_WORKSPACE_ID, &child)
        .unwrap();
    assert_eq!(links.len(), usize::from(explicit_workdir));
    if let Some(id) = workdir.lock().unwrap().as_ref() {
        assert_eq!(
            fs::read_to_string(
                harness
                    .materializer
                    .bind_working_directory(id, None)
                    .unwrap()
                    .root()
                    .join("README.md")
            )
            .unwrap(),
            "clean\n"
        );
    }
    api.store
        .revoke_worker_control_grant(
            TEST_WORKSPACE_ID,
            &forward.grant_id,
            &now_registry_timestamp(),
        )
        .unwrap();
    let (denied, body) = connected_authorization_post(
        &harness.backend_url,
        &harness.identity,
        &origin,
        &format!(
            "/api/w/{TEST_WORKSPACE_ID}/worker-control/workers/{}/{}/input",
            child.runtime_id, child.worker_id
        ),
        &json!({"kind":"notify","content":"MUST_NOT_ACCEPT_REVOKED_CONTROL"}),
    )
    .await;
    assert_eq!(
        denied,
        StatusCode::NOT_FOUND,
        "revoked forward control: {body}"
    );
    assert_eq!(
        harness.runtime.worker_detail(&child_ref).unwrap().status,
        worker_runtime::catalog::WorkerStatus::Stopped
    );
    assert!(
        !serde_json::to_string(
            &harness
                .factory
                .retained_session_snapshot(&child_ref)
                .unwrap()
                .snapshot
        )
        .unwrap()
        .contains("MUST_NOT_ACCEPT_REVOKED_CONTROL")
    );
    let calls = harness.authorizations.lock().unwrap();
    for op in [
        "worker.create",
        "worker.messaging",
        "worker.stop",
        "worker.observe",
    ] {
        assert!(
            calls
                .iter()
                .any(|(request, status)| request["operation"] == op && *status == StatusCode::OK),
            "real signed common authorization for {op}: {calls:?}"
        );
    }
    assert!(
        calls
            .iter()
            .filter(|(request, _)| request["operation"] == "worker.messaging"
                || request["operation"] == "worker.stop")
            .all(|(request, _)| request["resource"]["kind"] == "worker_operation")
    );
    assert!(
        browser_ticket_backend(api)
            .unwrap()
            .list(Default::default())
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn public_spawn_notify_reply_and_stop_without_ticket_or_attachment() {
    connected_roundtrip(false, false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn public_spawn_notify_reply_and_stop_with_explicit_workdir() {
    connected_roundtrip(true, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn public_initial_reply_unavailable_until_original_spawn_replay_repairs_grant() {
    connected_roundtrip(false, true).await;
}
