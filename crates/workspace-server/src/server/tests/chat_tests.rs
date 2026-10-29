//! Chat boundary tests (`cargo test -p yoi-workspace-server --lib chat_tests`).
//! Router authentication, Subject policy/singleton and FeatureStorage SQLite are
//! real. Component cases reuse Fixture's fake execution boundary, never claiming
//! controller/history evidence from it. The connected case below runs the real
//! HTTP Backend + Runtime + ProfileRuntimeWorkerFactory; only the external model
//! is scripted. All endpoints and storage are ephemeral; no live model is used.
use super::*;
use server_api::{ChatConversation, ChatMessagePage, ChatReceiveResponse};

fn conversations_path() -> String {
    format!("/api/w/{TEST_WORKSPACE_ID}/chat/conversations")
}
fn messages_path(conversation: &str) -> String {
    format!("{}/{conversation}/messages", conversations_path())
}
fn chat_worker_path() -> String {
    format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/chat/worker")
}
async fn account(
    api: &WorkspaceApi,
    owner: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    response(api, human_request(owner, method, path, body)).await
}
async fn conversation(f: &Fixture, subject: &str, operation: &str) -> ChatConversation {
    let (status, body) = account(
        &f.api,
        &f.owner,
        "POST",
        &conversations_path(),
        Some(json!({"operation_id":operation,"subject_id":subject})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}
async fn user(f: &Fixture, conversation: &str, operation: &str, text: &str) -> ChatReceiveResponse {
    let (status, body) = account(
        &f.api,
        &f.owner,
        "POST",
        &messages_path(conversation),
        Some(json!({"operation_id":operation,"text":text})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}
async fn page(f: &Fixture, conversation: &str) -> ChatMessagePage {
    let (status, body) = account(&f.api, &f.owner, "GET", &messages_path(conversation), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_value(body).unwrap()
}
async fn worker_chat(
    f: &Fixture,
    source: &RuntimeWorkerRef,
    operation: Value,
) -> (StatusCode, Value) {
    response(
        &f.api,
        runtime_source_request(
            &f.identity,
            Some(&source.worker_id),
            "POST",
            &chat_worker_path(),
            serde_json::to_vec(&json!({"operation":operation})).unwrap(),
        ),
    )
    .await
}
fn read(conversation: &str) -> Value {
    json!({"kind":"read","conversation_id":conversation,"query":{}})
}
fn send(conversation: &str, operation: &str, last_user_message_id: Option<&str>) -> Value {
    json!({"kind":"send","request":{"conversation_id":conversation,"operation_id":operation,
        "text":"explicit Subject reply","expected_last_user_message_id":last_user_message_id}})
}
async fn allow_chat(f: &Fixture) {
    f.assign(
        ["chat.read", "chat.send"]
            .into_iter()
            .map(|op| subject_permission(&f.subject, op))
            .collect(),
        vec![],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chat_account_routes_require_account_and_worker_route_requires_signed_current_body() {
    let f = Fixture::new().await;
    allow_chat(&f).await;
    let c = conversation(&f, &f.subject, "auth-conversation").await;
    let routes = [
        (
            "POST",
            conversations_path(),
            json!({"operation_id":"forged","subject_id":f.subject}),
        ),
        (
            "POST",
            messages_path(&c.id),
            json!({"operation_id":"forged","text":"not user input"}),
        ),
        ("GET", messages_path(&c.id), json!(null)),
    ];
    let stranger = seed_test_api_token(f.api.store.as_ref(), "chat-foreign-account");
    for (method, path, body) in routes {
        let bytes = if method == "GET" {
            vec![]
        } else {
            serde_json::to_vec(&body).unwrap()
        };
        let (status, error) = response(
            &f.api,
            runtime_source_request(&f.identity, Some(&f.source.worker_id), method, &path, bytes),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "Worker cannot impersonate account: {error}"
        );
        let body = (method != "GET").then_some(body);
        assert_eq!(
            account(&f.api, &stranger, method, &path, body.clone())
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        let unsigned = Request::builder()
            .method(method)
            .uri(&path)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.map(|b| b.to_string()).unwrap_or_default()))
            .unwrap();
        assert_eq!(response(&f.api, unsigned).await.0, StatusCode::UNAUTHORIZED);
    }
    let wire = json!({"operation":read(&c.id)});
    assert_eq!(
        account(
            &f.api,
            &f.owner,
            "POST",
            &chat_worker_path(),
            Some(wire.clone())
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, error) = response(
        &f.api,
        runtime_source_request(
            &f.identity,
            None,
            "POST",
            &chat_worker_path(),
            serde_json::to_vec(&wire).unwrap(),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "Runtime without Worker: {error}"
    );
    let mut tampered = runtime_source_request(
        &f.identity,
        Some(&f.source.worker_id),
        "POST",
        &chat_worker_path(),
        serde_json::to_vec(&wire).unwrap(),
    );
    *tampered.body_mut() = Body::from(json!({"operation":send(&c.id,"forged", None)}).to_string());
    assert_eq!(response(&f.api, tampered).await.0, StatusCode::UNAUTHORIZED);
    assert!(page(&f, &c.id).await.messages.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chat_user_replay_keeps_one_public_message_and_stable_notify_identity() {
    let f = Fixture::new().await;
    let c = conversation(&f, &f.subject, "create-once").await;
    assert_eq!(conversation(&f, &f.subject, "create-once").await, c);
    let first = user(&f, &c.id, "user-once", "please investigate").await;
    let replay = user(&f, &c.id, "user-once", "please investigate").await;
    assert_eq!(first.message, replay.message);
    assert_eq!(
        first.message.sender,
        server_api::ChatMessageSender::User {
            account_id: c.participant_account_id.clone()
        }
    );
    assert_eq!(
        first.delivery,
        server_api::ChatDelivery::Accepted {
            runtime_id: f.source.runtime_id.clone(),
            worker_id: f.source.worker_id.clone()
        }
    );
    // This is transport conformance only. Fake Runtime acceptance is not a
    // durable controller receipt, incorporated history or model processing.
    let requests = f.runtime.input_requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].submission_request_id,
        requests[1].submission_request_id
    );
    assert_eq!(requests[0].content, requests[1].content);
    assert!(
        requests[0]
            .submission_request_id
            .as_deref()
            .unwrap()
            .starts_with("runtime-input:notify:")
    );
    assert_eq!(
        requests[0].source,
        Some(protocol::AuthenticatedInputSource::Account {
            account_id: c.participant_account_id.clone()
        })
    );
    assert_eq!(requests[0].kind, WorkerInputKind::Notify);
    assert!(requests[0].segments.is_none());
    assert!(requests[0].content.contains("not Host instructions"));
    let generic_path = format!(
        "/api/w/{TEST_WORKSPACE_ID}/runtimes/{}/workers/{}/input",
        f.source.runtime_id, f.source.worker_id
    );
    let (status, result) = account(
        &f.api,
        &f.owner,
        "POST",
        &generic_path,
        Some(
            json!({"kind":"notify", "content":"ordinary account Notify, not public chat",
            "submission_request_id":format!("chat-message:{}",first.message.id)}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let generic = f
        .runtime
        .input_requests
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_ne!(
        generic.submission_request_id, requests[0].submission_request_id,
        "a caller-chosen public Notify ID cannot consume a chat receipt"
    );
    assert_eq!(generic.source, requests[0].source);
    let (status, error) = account(
        &f.api,
        &f.owner,
        "POST",
        &messages_path(&c.id),
        Some(json!({"operation_id":"user-once","text":"changed"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    let saved = page(&f, &c.id).await;
    assert_eq!(
        saved.conversation.last_user_message_id.as_deref(),
        Some(first.message.id.as_str())
    );
    assert_eq!(
        first.message.last_user_message_id.as_deref(),
        Some(first.message.id.as_str())
    );
    assert_eq!(saved.messages, vec![first.message]);
    assert_eq!(
        f.runtime.input_requests.lock().unwrap().len(),
        3,
        "conflict must not notify"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chat_text_only_http_boundary_rejects_claimed_identity_attachments_and_oversize_without_append()
 {
    let f = Fixture::new().await;
    allow_chat(&f).await;
    let c = conversation(&f, &f.subject, "text-only").await;
    for field in [
        "attachments",
        "segments",
        "sender",
        "subject_id",
        "account_id",
    ] {
        let mut input = json!({"operation_id":"invalid-user","text":"text"});
        input[field] = json!(["/private/not-an-attachment"]);
        let (status, error) =
            account(&f.api, &f.owner, "POST", &messages_path(&c.id), Some(input)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{field}: {error}");
        let mut input = send(&c.id, "invalid-send", None);
        input["request"][field] = json!("claimed");
        let (status, error) = worker_chat(&f, &f.source, input).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{field}: {error}");
    }
    for text in [
        String::new(),
        "x".repeat(server_api::CHAT_MAX_TEXT_BYTES + 1),
    ] {
        let (status, error) = account(
            &f.api,
            &f.owner,
            "POST",
            &messages_path(&c.id),
            Some(json!({"operation_id":"invalid-size","text":text})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    }
    let mut missing_baseline = send(&c.id, "missing-baseline", None);
    missing_baseline["request"]
        .as_object_mut()
        .unwrap()
        .remove("expected_last_user_message_id");
    let (status, error) = worker_chat(&f, &f.source, missing_baseline).await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "must explicitly observe null for an empty conversation: {error}"
    );
    assert!(page(&f, &c.id).await.messages.is_empty());
    assert!(f.runtime.input_requests.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chat_explicit_subject_send_replays_and_pages_public_messages_with_exact_user_message_baseline()
 {
    let f = Fixture::new().await;
    allow_chat(&f).await;
    let c = conversation(&f, &f.subject, "subject-send").await;
    let input = user(&f, &c.id, "input-one", "question").await;
    let (status, read_page) = worker_chat(&f, &f.source, read(&c.id)).await;
    assert_eq!(status, StatusCode::OK, "{read_page}");
    assert_eq!(
        read_page["page"]["conversation"]["last_user_message_id"],
        input.message.id
    );
    let mut request = send(&c.id, "reply-once", Some(&input.message.id));
    request["request"]["reply_to"] = json!(input.message.id);
    request["request"]["work_ref"] = json!("opaque-work-reference");
    let (status, sent) = worker_chat(&f, &f.source, request.clone()).await;
    assert_eq!(status, StatusCode::OK, "{sent}");
    assert_eq!(
        sent["message"]["sender"],
        json!({"kind":"subject","subject_id":f.subject})
    );
    assert_eq!(
        worker_chat(&f, &f.source, request.clone()).await,
        (StatusCode::OK, sent.clone())
    );
    let mut changed = request.clone();
    changed["request"]["text"] = json!("not the original");
    assert_eq!(
        worker_chat(&f, &f.source, changed).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(sent["message"]["last_user_message_id"], input.message.id);
    let later = user(&f, &c.id, "input-two", "additional instruction").await;
    // Empty and prior observed User identities are both stale after an append.
    for baseline in [None, Some(input.message.id.as_str())] {
        assert_eq!(
            worker_chat(&f, &f.source, send(&c.id, "stale-new-send", baseline))
                .await
                .0,
            StatusCode::CONFLICT
        );
    }
    let mut replay_with_new_baseline = request.clone();
    replay_with_new_baseline["request"]["expected_last_user_message_id"] = json!(later.message.id);
    assert_eq!(
        worker_chat(&f, &f.source, replay_with_new_baseline).await.0,
        StatusCode::CONFLICT,
        "replay compares the saved baseline as part of the complete request"
    );
    // A response lost after commit is recovered even after a newer user input.
    assert_eq!(
        worker_chat(&f, &f.source, request).await,
        (StatusCode::OK, sent)
    );
    assert_eq!(
        worker_chat(
            &f,
            &f.source,
            send(&c.id, "current-send", Some(&later.message.id))
        )
        .await
        .0,
        StatusCode::OK
    );
    let saved = page(&f, &c.id).await;
    assert_eq!(saved.messages.len(), 4);
    assert_eq!(
        saved.conversation.last_user_message_id.as_deref(),
        Some(later.message.id.as_str())
    );
    let (status, first_page) = account(
        &f.api,
        &f.owner,
        "GET",
        &format!("{}?limit=1", messages_path(&c.id)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first_page}");
    assert_eq!(first_page["messages"].as_array().unwrap().len(), 1);
    assert_eq!(first_page["has_more"], true);
    let (status, rest) = account(
        &f.api,
        &f.owner,
        "GET",
        &format!("{}?after_sequence=1&limit=100", messages_path(&c.id)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{rest}");
    assert_eq!(rest["messages"].as_array().unwrap().len(), 3);
    assert_eq!(rest["has_more"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chat_live_policy_revoke_denies_new_send_and_replay_and_read_independently() {
    let f = Fixture::new().await;
    let c = conversation(&f, &f.subject, "revoke").await;
    assert_eq!(
        worker_chat(&f, &f.source, read(&c.id)).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        worker_chat(&f, &f.source, send(&c.id, "denied", None))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    allow_chat(&f).await;
    let sent = send(&c.id, "before-revoke", None);
    assert_eq!(
        worker_chat(&f, &f.source, sent.clone()).await.0,
        StatusCode::OK
    );
    f.assign(vec![subject_permission(&f.subject, "chat.read")], vec![])
        .await;
    assert_eq!(
        worker_chat(&f, &f.source, read(&c.id)).await.0,
        StatusCode::OK
    );
    for request in [sent, send(&c.id, "after-revoke", None)] {
        assert_eq!(
            worker_chat(&f, &f.source, request).await.0,
            StatusCode::FORBIDDEN
        );
    }
    f.assign(vec![subject_permission(&f.subject, "chat.send")], vec![])
        .await;
    assert_eq!(
        worker_chat(&f, &f.source, read(&c.id)).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(page(&f, &c.id).await.messages.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chat_cross_subject_background_and_released_body_cannot_read_or_publish() {
    let f = Fixture::new().await;
    let other = open_subjektiv_store(&f.api)
        .unwrap()
        .create_subject(crate::subjektiv::SubjectRole::new("other-chat").unwrap())
        .unwrap()
        .id;
    let c = conversation(&f, &f.subject, "own-conversation").await;
    let foreign = conversation(&f, &other, "foreign-conversation").await;
    let permissions = ["chat.read", "chat.send"]
        .into_iter()
        .map(|op| subject_permission(&f.subject, op))
        .collect::<Vec<_>>();
    f.assign(permissions.clone(), permissions.clone()).await;
    let child = f.child("background-chat", permissions).await;
    for source in [&child, &f.source] {
        let target = if source == &child { &c.id } else { &foreign.id };
        for request in [read(target), send(target, "not-authorized", None)] {
            let (status, error) = worker_chat(&f, source, request).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{error}");
        }
    }
    f.release_source_lease();
    for request in [read(&c.id), send(&c.id, "old-body", None)] {
        let (status, error) = worker_chat(&f, &f.source, request).await;
        // Normal source authentication revokes the released body before the
        // route's own current-body check; neither read nor Send reaches storage.
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{error}");
    }
    assert!(page(&f, &c.id).await.messages.is_empty());
    assert!(page(&f, &foreign.id).await.messages.is_empty());
    let received = user(&f, &c.id, "without-body", "saved, not activated").await;
    assert_eq!(received.delivery, server_api::ChatDelivery::NoCurrentBody);
    assert_eq!(page(&f, &c.id).await.messages, vec![received.message]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chat_subject_owner_writer_fence_covers_feature_commit_and_rejects_other_subject_or_released_lease()
 {
    let f = Fixture::new().await;
    allow_chat(&f).await;
    let c = conversation(&f, &f.subject, "owner-fence").await;
    let lease = f
        .api
        .store
        .require_current_worker_singleton_owner(TEST_WORKSPACE_ID, &f.source)
        .unwrap()
        .unwrap();
    let db_path = f
        .api
        .config_store
        .with_conn(|conn| Ok(conn.path().unwrap().to_owned()))
        .unwrap();
    let other = rusqlite::Connection::open(db_path).unwrap();
    other.busy_timeout(std::time::Duration::ZERO).unwrap();
    let store = open_subjektiv_store(&f.api).unwrap();
    let policy = store.subject_permissions(&f.subject).unwrap().unwrap();
    let request =
        serde_json::from_value(send(&c.id, "fenced-send", None)["request"].clone()).unwrap();
    let message = f
        .api
        .config_store
        .with_current_subject_body(TEST_WORKSPACE_ID, &f.subject, &lease, || {
            // This independent Backend connection cannot release ownership between
            // the current-owner observation and the real FeatureStorage commit.
            let release = other.execute(
                "DELETE FROM worker_singleton_owners WHERE workspace_id = ?1",
                rusqlite::params![TEST_WORKSPACE_ID],
            );
            assert!(
                matches!(release, Err(rusqlite::Error::SqliteFailure(error, _))
            if error.code == rusqlite::ErrorCode::DatabaseBusy)
            );
            Ok(store
                .chat_send_message_authorized(&f.subject, &request, &policy.change_id)
                .unwrap())
        })
        .unwrap();
    assert_eq!(page(&f, &c.id).await.messages, vec![message]);
    assert!(
        f.api
            .config_store
            .with_current_subject_body::<()>(TEST_WORKSPACE_ID, "other-subject", &lease, || {
                panic!("another Subject's lease must not reach publication")
            })
            .is_err()
    );
    f.release_source_lease();
    assert!(
        f.api
            .config_store
            .with_current_subject_body::<()>(TEST_WORKSPACE_ID, &f.subject, &lease, || {
                panic!("released lease must not reach publication")
            })
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chat_sqlite_append_failure_returns_no_success_or_notify_and_rolls_back_user_baseline() {
    let f = Fixture::new().await;
    allow_chat(&f).await;
    let c = conversation(&f, &f.subject, "failed-append").await;
    // Fail the real SQLite append, after all route authentication/authorization.
    let storage = f
        .api
        .feature_storage
        .open(&f.api.subjektiv_registration)
        .unwrap();
    storage.with_connection(|db| {
        db.execute_batch("CREATE TRIGGER fail_chat_append BEFORE INSERT ON chat_messages BEGIN SELECT RAISE(ABORT, 'injected chat append failure'); END;")?;
        Ok(())
    }).unwrap();
    let (status, error) = account(
        &f.api,
        &f.owner,
        "POST",
        &messages_path(&c.id),
        Some(json!({"operation_id":"fail-user","text":"must rollback"})),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{error}");
    let (status, error) = worker_chat(&f, &f.source, send(&c.id, "fail-subject", None)).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{error}");
    let saved = page(&f, &c.id).await;
    assert_eq!(saved.conversation.last_user_message_id, None);
    assert!(saved.messages.is_empty());
    assert!(f.runtime.input_requests.lock().unwrap().is_empty());
    storage
        .with_connection(|db| {
            db.execute_batch("DROP TRIGGER fail_chat_append;")?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        worker_chat(&f, &f.source, send(&c.id, "fail-subject", None))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        user(&f, &c.id, "fail-user", "must rollback")
            .await
            .message
            .sequence,
        2
    );
}

/// Only the model is scripted. ConversationRead/Send execute through the real
/// installed Feature, signed Runtime request transport, Backend and SQLite.
#[derive(Clone)]
struct ChatModel {
    calls: Arc<std::sync::atomic::AtomicUsize>,
    conversation: Arc<Mutex<String>>,
    inputs: tokio::sync::mpsc::Sender<(usize, String)>,
    after_busy_boundary: Arc<tokio::sync::Semaphore>,
}
#[async_trait::async_trait]
impl agen::llm_client::LlmClient for ChatModel {
    async fn stream(
        &self,
        request: agen::llm_client::Request,
    ) -> std::result::Result<agen::llm_client::ResponseStream, agen::llm_client::ClientError> {
        use agen::llm_client::{
            event::{Event, StopReason},
            types::Item,
        };
        let phase = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // Observe model messages separately from ToolResult bodies: the first
        // Read legitimately repeats user text as its returned public page.
        let messages = request
            .items
            .iter()
            .filter(|item| matches!(item, Item::Message { .. }))
            .collect::<Vec<_>>();
        let _ = self.inputs.try_send((phase, format!("{messages:?}")));
        if phase == 1 {
            tokio::time::timeout(
                std::time::Duration::from_secs(30),
                self.after_busy_boundary.acquire(),
            )
            .await
            .expect("committed history observation deadline")
            .expect("test boundary semaphore")
            .forget();
            assert!(request.items.iter().any(|item| matches!(item, Item::ToolResult { call_id, is_error: false, .. } if call_id == "chat-tool-0")), "the gated real Read must return normally, not be interrupted by Notify");
        }
        assert!(
            !request
                .tools
                .iter()
                .any(|t| matches!(t.name.as_str(), "Bash" | "Write" | "Edit")),
            "chat body must not expose editing/command Tools"
        );
        for item in &request.items {
            if let Item::ToolResult {
                is_error, content, ..
            } = item
            {
                assert!(!is_error, "real connected chat Tool failed: {content:?}");
            }
        }
        let events = match phase {
            0..=3 => {
                let conversation = self.conversation.lock().unwrap().clone();
                let (name, input) = if phase <= 1 {
                    // The first Read response was computed before the Busy
                    // inputs arrived. Reread at the next safe boundary instead
                    // of guessing a baseline from notifications.
                    ("ConversationRead", json!({"conversation_id":conversation}))
                } else {
                    let page = request
                        .items
                        .iter()
                        .find_map(|item| match item {
                            Item::ToolResult {
                                call_id,
                                content: Some(content),
                                ..
                            } if call_id == "chat-tool-1" => Some(
                                serde_json::from_str::<server_api::ChatMessagePage>(content)
                                    .expect("actual ConversationRead JSON"),
                            ),
                            _ => None,
                        })
                        .expect("Send must use the real refreshed ConversationRead result");
                    assert_eq!(page.conversation.id, conversation);
                    let baseline = page.conversation.last_user_message_id;
                    assert!(
                        baseline.is_some(),
                        "actual User messages precede the refreshed Read"
                    );
                    (
                        "ConversationSend",
                        json!({"conversation_id":conversation,"operation_id":format!("connected-send-{phase}"),"text":format!("public reply {phase}"),"expected_last_user_message_id":baseline}),
                    )
                };
                assert!(
                    request.tools.iter().any(|tool| tool.name == name),
                    "builtin:async-chat must expose {name}"
                );
                vec![
                    Event::tool_use_start(0, format!("chat-tool-{phase}"), name),
                    Event::tool_input_delta(0, input.to_string()),
                    Event::tool_use_stop(0),
                ]
            }
            4..=6 => vec![
                Event::text_block_start(0),
                Event::text_delta(0, "INTERNAL_ASSISTANT_TEXT_NOT_A_PUBLIC_CHAT_MESSAGE"),
                Event::text_block_stop(0, Some(StopReason::EndTurn)),
            ],
            other => panic!(
                "unexpected model request {other}; notification replay must not activate twice"
            ),
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
    fn clone_boxed(&self) -> Box<dyn agen::llm_client::LlmClient> {
        Box::new(self.clone())
    }
}

/// Reuse the parent suite's RAII HTTP servers and signing helper. The setup
/// below is the minimal workdirless subset of its connected Runtime fixture;
/// no repository, child grant or local-tool setup is duplicated here.
struct ConnectedChat {
    api: WorkspaceApi,
    runtime: worker_runtime::Runtime,
    owner: String,
    url: String,
    lose_next_input_ack: Arc<std::sync::atomic::AtomicBool>,
    _backend: AuthorizationCallbackServer,
    _runtime_server: ConnectedRuntimeServer,
    _dir: tempfile::TempDir,
}
fn publish_chat_fixture_profile(api: &WorkspaceApi) {
    // A bundled Profile is not automatically a Backend-published candidate.
    // Publish the actual built-in without copying or widening its capabilities.
    let state = api
        .config_store
        .load_workspace_config(TEST_WORKSPACE_ID)
        .unwrap()
        .unwrap();
    let main = state
        .snapshot
        .get(&config_source::VirtualPath::parse("main.dcdl").unwrap())
        .unwrap();
    let content = r#"{ profile = { default_profile = "project:connected-chat"; entries = [{ selector = "project:connected-chat"; label = "Connected chat"; description = "Real built-in async chat fixture"; profile = import "$builtin/profiles/async-chat.dcdl"; }]; }; }"#;
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

impl ConnectedChat {
    async fn new(
        model: impl agen::llm_client::LlmClient + Clone + 'static,
        app_layer: impl FnOnce(Router) -> Router,
    ) -> Self {
        const RUNTIME: &str = "connected-chat";
        let dir = tempfile::tempdir().unwrap();
        let backend_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let runtime_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", backend_listener.local_addr().unwrap());
        let runtime_url = format!("http://{}", runtime_listener.local_addr().unwrap());
        let identity = RuntimeIdentityMaterial::generate(RUNTIME).unwrap();
        let root = dir.path().join("runtime");
        let factory = worker_runtime::worker_backend::ProfileRuntimeWorkerFactory::new(
            dir.path().join("profiles"),
        )
        .with_remote_worker_mutation_identity(identity.clone())
        .with_workspace_request_client(
            worker_runtime::workspace_request::RuntimeWorkspaceRequestClient::new(
                TEST_WORKSPACE_ID,
                &url,
                RUNTIME,
            )
            .with_runtime_request_source(&identity, &url),
        )
        .with_runtime_store_dir(&root)
        .with_spawn_model_client_factory(move || Box::new(model.clone()));
        let execution =
            worker_runtime::worker_backend::WorkerRuntimeExecutionBackend::new(factory).unwrap();
        let runtime = worker_runtime::Runtime::with_fs_store_and_execution_backend(
            worker_runtime::fs_store::FsRuntimeStoreOptions {
                root,
                runtime_id: RUNTIME.into(),
                display_name: None,
            },
            Arc::new(execution),
        )
        .unwrap();
        let mut config = test_server_config(dir.path());
        config.backend_base_url = Some(url.clone());
        let control = Arc::new(test_control_store(&config));
        seed_test_registered_workspace(control.as_ref(), &config)
            .await
            .unwrap();
        let api = WorkspaceApi::new_with_execution_backend(
            config,
            control,
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .unwrap();
        let now = now_registry_timestamp();
        api.store
            .upsert_workspace_runtime_binding_record(
                WorkspaceRuntimeBinding {
                    workspace_id: TEST_WORKSPACE_ID.into(),
                    runtime_id: RUNTIME.into(),
                    display_name: RUNTIME.into(),
                    base_url: runtime_url.clone(),
                    public_key: identity.public_key.clone(),
                    public_key_fingerprint: String::new(),
                    binding_id: "connected-chat-binding".into(),
                    authentication_mode: StoredRuntimeAuthenticationMode::WorkspaceIdentity,
                    created_at: now.clone(),
                    updated_at: now,
                    revoked_at: None,
                },
                false,
            )
            .await
            .unwrap();
        let binding = api
            .store
            .get_workspace_runtime_binding(TEST_WORKSPACE_ID, RUNTIME)
            .await
            .unwrap()
            .unwrap();
        let workspace_identity = api
            .signing_identities
            .provision_existing(TEST_WORKSPACE_ID, &test_owner_actor().account_id)
            .unwrap();
        let verifier = worker_runtime::workspace_issuer::WorkspaceCapabilityVerifier::new(
            vec![
                worker_runtime::workspace_issuer::WorkspaceIssuerTrustRecord {
                    workspace_id: TEST_WORKSPACE_ID.into(),
                    backend_url: url.clone(),
                    key_id: workspace_identity.key_id.clone(),
                    algorithm: workspace_identity.algorithm.clone(),
                    public_key: workspace_identity.public_key.clone().unwrap(),
                    public_key_fingerprint: workspace_identity
                        .public_key_fingerprint
                        .clone()
                        .unwrap(),
                    trust_id: "connected-chat-trust".into(),
                    state: worker_runtime::workspace_issuer::WorkspaceIssuerTrustState::Active,
                    registered_at_unix: 1,
                    updated_at_unix: 1,
                },
            ],
            Arc::new(
                worker_runtime::workspace_issuer::InMemoryWorkspaceClaimReplayProtection::default(),
            ),
        )
        .unwrap();
        let lose_next_input_ack = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ack_fault = lose_next_input_ack.clone();
        let runtime_router = worker_runtime::http_server::runtime_http_router_with_workspace_auth(
            runtime.clone(),
            None,
            worker_runtime::http_server::WorkspaceRuntimeHttpAuth {
                verifier,
                runtime_id: RUNTIME.into(),
            },
        )
        .layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let ack_fault = ack_fault.clone();
                async move {
                    let lose = request.uri().path().ends_with("/input")
                        && ack_fault.swap(false, std::sync::atomic::Ordering::SeqCst);
                    let result = next.run(request).await;
                    if lose {
                        if !result.status().is_success() {
                            let status = result.status();
                            let body = to_bytes(result.into_body(), 2 * 1024 * 1024).await.unwrap();
                            panic!("cannot inject ACK loss before actual Runtime acceptance: {status} {}",String::from_utf8_lossy(&body));
                        }
                        (
                            StatusCode::BAD_GATEWAY,
                            "injected lost Notify acknowledgement",
                        )
                            .into_response()
                    } else {
                        result
                    }
                }
            },
        ));
        let runtime_server = ConnectedRuntimeServer::start(runtime_listener, runtime_router);
        api.runtime.register_or_replace(
            RemoteWorkerRuntime::new(
                RemoteRuntimeConfig {
                    runtime_id: RUNTIME.into(),
                    workspace_id: Some(TEST_WORKSPACE_ID.into()),
                    display_name: RUNTIME.into(),
                    base_url: runtime_url,
                    bearer_token: None,
                    workspace_authorization: Some(
                        crate::hosts::WorkspaceRuntimeAuthorization::new(
                            api.store.clone(),
                            api.signing_identities.clone(),
                            url.clone(),
                            binding,
                        ),
                    ),
                    strict_public_egress: false,
                    cached_worker_creation_available: true,
                    cached_os: "linux".into(),
                    cached_arch: "x86_64".into(),
                    cached_status: "active".into(),
                    timeout: std::time::Duration::from_secs(30),
                },
                TEST_WORKSPACE_ID.to_string(),
                url.clone(),
            )
            .unwrap(),
        );
        publish_chat_fixture_profile(&api);
        seed_test_delivery_binding(&api);
        let owner =
            seed_test_workspace_owner_token(api.store.as_ref(), "connected-chat-owner").await;
        let backend = AuthorizationCallbackServer::start(
            backend_listener,
            app_layer(build_router(api.clone())),
        );
        Self {
            api,
            runtime,
            owner,
            url,
            lose_next_input_ack,
            _backend: backend,
            _runtime_server: runtime_server,
            _dir: dir,
        }
    }
    async fn post(&self, path: &str, body: Value) -> Value {
        let response = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap()
            .post(format!("{}{path}", self.url))
            .bearer_auth(&self.owner)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body: Value = response.json().await.unwrap();
        assert!(status.is_success(), "{path}: {status} {body}");
        body
    }
}

async fn next_chat_input(
    inputs: &mut tokio::sync::mpsc::Receiver<(usize, String)>,
    expected: usize,
) -> String {
    let (phase, input) = tokio::time::timeout(std::time::Duration::from_secs(30), inputs.recv())
        .await
        .expect("connected model input deadline")
        .expect("model input sender");
    assert_eq!(phase, expected);
    input
}
async fn chat_idle(
    events: &mut tokio::sync::broadcast::Receiver<
        worker_runtime::observation::WorkerObservationEvent,
    >,
    worker: &str,
) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let event = events.recv().await.expect("Runtime protocol observation");
            if event.worker_ref.worker_id.to_string() == worker && matches!(event.payload,protocol::Event::WorkerState { snapshot } if snapshot.state == protocol::WorkerState::Idle) { break; }
        }
    }).await.expect("real controller Idle event deadline");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn chat_public_receive_busy_messages_commit_before_next_model_input_and_idle_notify_activates()
 {
    let (inputs_tx, mut inputs) = tokio::sync::mpsc::channel(8);
    let conversation_id = Arc::new(Mutex::new(String::new()));
    let after_busy_boundary = Arc::new(tokio::sync::Semaphore::new(0));
    let model = ChatModel {
        calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        conversation: conversation_id.clone(),
        inputs: inputs_tx,
        after_busy_boundary: after_busy_boundary.clone(),
    };
    let calls = model.calls.clone();
    let (read_started, mut read_events) = tokio::sync::mpsc::channel(1);
    let (release, read_release) = tokio::sync::oneshot::channel::<()>();
    let read_release = Arc::new(Mutex::new(Some(read_release)));
    let f = ConnectedChat::new(model, move |router| {
        router.layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let read_started = read_started.clone();
                let read_release = read_release.clone();
                async move {
                    let gate = if request.uri().path() == chat_worker_path() {
                        read_release.lock().unwrap().take()
                    } else {
                        None
                    };
                    // Hold the real first ConversationRead HTTP response. A Notify must
                    // not cancel this Tool or cause a competing model invocation.
                    let response = next.run(request).await;
                    if let Some(gate) = gate {
                        let _ = read_started.send(response.status()).await;
                        tokio::time::timeout(std::time::Duration::from_secs(30), gate)
                            .await
                            .expect("test must release the real Tool response")
                            .unwrap();
                    }
                    response
                }
            },
        ))
    })
    .await;
    let store = open_subjektiv_store(&f.api).unwrap();
    let subject = store
        .create_subject(crate::subjektiv::SubjectRole::new("connected-chat").unwrap())
        .unwrap()
        .id;
    let operations = [
        "session.record",
        "memory.resident.read",
        "memory.resident.supply",
        "journal.startup.read",
        "journal.startup.supply",
        // Restore uses the ordinary fail-closed recording hook as well.
        "journal.recording.request",
        "chat.read",
        "chat.send",
    ];
    f.post(&format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{subject}/permissions"),json!({"operation_id":"connected-chat-policy","expected_change_id":null,"execution":operations.into_iter().map(|op|subject_permission(&subject,op)).collect::<Vec<_>>(),"delegation":[]})).await;
    let conversation = f
        .post(
            &conversations_path(),
            json!({"operation_id":"connected-conversation","subject_id":subject}),
        )
        .await;
    let c = conversation["id"].as_str().unwrap();
    *conversation_id.lock().unwrap() = c.into();
    let created = f.post(&format!("/api/w/{TEST_WORKSPACE_ID}/workers"),json!({"runtime_id":"connected-chat","display_name":"Connected async chat","profile":"project:connected-chat","initial_submit":[],"workdir_attachments":[],"feature_connections":{"subjektiv":{"subject_id":subject}}})).await;
    let worker = created["worker_id"].as_str().unwrap();
    let mut events = f.runtime.subscribe_worker_observation().unwrap();
    let first_wire = json!({"operation_id":"connected-first","text":"CHAT_FIRST_USER_CONTENT"});
    f.lose_next_input_ack
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let first = f.post(&messages_path(c), first_wire.clone()).await;
    assert_eq!(
        first["delivery"]["state"], "unknown",
        "lost Runtime ACK is not reported as rejection or success"
    );
    next_chat_input(&mut inputs, 0).await;
    let status = tokio::time::timeout(std::time::Duration::from_secs(30), read_events.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "actual ConversationRead must succeed before gating its response"
    );
    let second = f
        .post(
            &messages_path(c),
            json!({"operation_id":"connected-busy-one","text":"CHAT_BUSY_ONE_CONTENT"}),
        )
        .await;
    let third = f
        .post(
            &messages_path(c),
            json!({"operation_id":"connected-busy-two","text":"CHAT_BUSY_TWO_CONTENT"}),
        )
        .await;
    for result in [&second, &third] {
        assert_eq!(result["delivery"]["state"], "accepted");
    }
    let replay = f.post(&messages_path(c), first_wire).await;
    assert_eq!(replay["message"], first["message"]);
    assert_eq!(replay["delivery"]["state"], "accepted");
    assert!(
        matches!(
            inputs.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ),
        "Busy Notify cannot interrupt the gated Tool"
    );
    release.send(()).unwrap();
    let after_tool = next_chat_input(&mut inputs, 1).await;
    for marker in [
        "CHAT_FIRST_USER_CONTENT",
        "CHAT_BUSY_ONE_CONTENT",
        "CHAT_BUSY_TWO_CONTENT",
    ] {
        assert_eq!(
            after_tool.matches(marker).count(),
            1,
            "committed notification appears once at the safe next-model boundary: {after_tool}"
        );
    }
    // The scripted stream is paused at this next-model invocation until the
    // independently reopened committed Session below has been checked.
    // Read the real committed Session log independently of model observation.
    // The turn-paged API groups by real User Submit, while this fixture only
    // uses Notify; use the read-only canonical snapshot, including system input.
    let history = session_store::read_retained_session_snapshot(
        &f._dir.path().join("runtime/workers").join(worker),
        &format!("worker-runtime-{worker}"),
        2 * 1024 * 1024,
    )
    .expect("read committed Session from the real Runtime aggregate");
    for marker in [
        "CHAT_FIRST_USER_CONTENT",
        "CHAT_BUSY_ONE_CONTENT",
        "CHAT_BUSY_TWO_CONTENT",
    ] {
        let notifications = history.snapshot.entries.iter().filter(|entry| matches!(&entry.data,
            protocol::SessionSnapshotEntryData::SystemItem { item_kind, content, .. } if item_kind == "notification" && content.contains(marker)
        )).collect::<Vec<_>>();
        assert_eq!(
            notifications.len(),
            1,
            "one committed Notify per stable chat message: {notifications:?}"
        );
    }
    after_busy_boundary.add_permits(1);
    next_chat_input(&mut inputs, 2).await;
    next_chat_input(&mut inputs, 3).await;
    next_chat_input(&mut inputs, 4).await;
    chat_idle(&mut events, worker).await;
    let (status, public) = account(&f.api, &f.owner, "GET", &messages_path(c), None).await;
    assert_eq!(status, StatusCode::OK, "{public}");
    assert_eq!(
        public["messages"].as_array().unwrap().len(),
        5,
        "three user messages and two explicit Sends, not internal output"
    );
    assert!(!public.to_string().contains("INTERNAL_ASSISTANT"));
    let idle_wire = json!({"operation_id":"connected-idle","text":"CHAT_IDLE_USER_CONTENT"});
    let idle = f.post(&messages_path(c), idle_wire.clone()).await;
    assert_eq!(idle["delivery"]["state"], "accepted");
    let idle_input = next_chat_input(&mut inputs, 5).await;
    assert!(
        idle_input.contains("CHAT_IDLE_USER_CONTENT"),
        "Idle Notify must reach a new real model invocation"
    );
    chat_idle(&mut events, worker).await;
    assert_eq!(
        f.post(&messages_path(c), idle_wire).await["message"],
        idle["message"]
    );
    let (_, public) = account(&f.api, &f.owner, "GET", &messages_path(c), None).await;
    assert_eq!(
        public["messages"].as_array().unwrap().len(),
        6,
        "no-Send Idle turn adds only its user message"
    );
    assert!(!public.to_string().contains("INTERNAL_ASSISTANT"));
    let worker_ref = RuntimeWorkerRef::new("connected-chat", worker);
    let lifecycle_path =
        format!("/api/w/{TEST_WORKSPACE_ID}/runtimes/connected-chat/workers/{worker}");
    let stopped = f.post(&format!("{lifecycle_path}/stop"), json!({})).await;
    assert_eq!(stopped["state"], "accepted");
    let stopped_wire =
        json!({"operation_id":"connected-stopped","text":"CHAT_STOPPED_SAVED_NOT_NOTIFIED"});
    let undelivered = f.post(&messages_path(c), stopped_wire.clone()).await;
    assert_ne!(
        undelivered["delivery"]["state"], "accepted",
        "saving while stopped is not Notify acceptance"
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        6,
        "receive must not implicitly restore a stopped body"
    );
    let request = server_api::WorkerRestoreRequest {
        expected_observation_token: f
            .api
            .runtime
            .worker(&worker_ref)
            .unwrap()
            .restore_observation_token
            .unwrap(),
        request_id: "explicit-chat-restore".into(),
    };
    let restored = f
        .post(
            &format!("{lifecycle_path}/restore"),
            serde_json::to_value(request).unwrap(),
        )
        .await;
    let restored: server_api::WorkerRestoreResponse = serde_json::from_value(restored).unwrap();
    assert_eq!(
        restored.result.state,
        server_api::WorkerRestoreState::Accepted,
        "{restored:?}"
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        6,
        "public chat storage is not a stopped-body mailbox"
    );
    let redelivered = f.post(&messages_path(c), stopped_wire).await;
    assert_eq!(redelivered["message"], undelivered["message"]);
    assert_eq!(redelivered["delivery"]["state"], "accepted");
    let mut observed_states = Vec::new();
    let restored_input = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            tokio::select! {
                input = inputs.recv() => {
                    let (phase, content) = input.expect("restored model sender");
                    assert_eq!(phase, 6);
                    break content;
                }
                event = events.recv() => {
                    let event = event.expect("restored Runtime observation");
                    if event.worker_ref.worker_id.to_string() != worker { continue; }
                    match event.payload {
                        protocol::Event::Error { code, message } => panic!("restored notification failed before model: {code:?}: {message}"),
                        protocol::Event::WorkerState { snapshot } => observed_states.push(snapshot.state),
                        _ => {}
                    }
                }
            }
        }
    }).await.unwrap_or_else(|_| panic!("restored model input deadline; observed states: {observed_states:?}"));
    assert_eq!(
        restored_input
            .matches("CHAT_STOPPED_SAVED_NOT_NOTIFIED")
            .count(),
        1
    );
    chat_idle(&mut events, worker).await;
    let (_, public) = account(&f.api, &f.owner, "GET", &messages_path(c), None).await;
    assert_eq!(
        public["messages"].as_array().unwrap().len(),
        7,
        "explicit retry after restore preserves one public User message"
    );
    assert!(!public.to_string().contains("INTERNAL_ASSISTANT"));
}

#[derive(Clone)]
struct NotifyObservationModel(tokio::sync::mpsc::Sender<String>);
#[async_trait::async_trait]
impl agen::llm_client::LlmClient for NotifyObservationModel {
    async fn stream(
        &self,
        request: agen::llm_client::Request,
    ) -> std::result::Result<agen::llm_client::ResponseStream, agen::llm_client::ClientError> {
        use agen::llm_client::event::{Event, StopReason};
        self.0.send(format!("{:?}", request.items)).await.unwrap();
        Ok(Box::pin(futures::stream::iter([
            Ok(Event::text_block_start(0)),
            Ok(Event::text_delta(0, "internal only")),
            Ok(Event::text_block_stop(0, Some(StopReason::EndTurn))),
        ])))
    }
    fn clone_boxed(&self) -> Box<dyn agen::llm_client::LlmClient> {
        Box::new(self.clone())
    }
}

/// Real authenticated Backend WS -> signed Runtime WS -> controller -> SQLite,
/// followed by the production Console admission tracker (Deno, the normal test
/// runner prerequisite). Only the model is scripted; no browser rendering claim.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn browser_notify_preserves_ack_id_limits_retry_and_console_next_draft() {
    let (tx, mut inputs) = tokio::sync::mpsc::channel(32);
    let f = ConnectedChat::new(NotifyObservationModel(tx), |router| router).await;
    let subject = open_subjektiv_store(&f.api)
        .unwrap()
        .create_subject(crate::subjektiv::SubjectRole::new("notify-console").unwrap())
        .unwrap()
        .id;
    let operations = [
        "session.record",
        "memory.resident.read",
        "memory.resident.supply",
        "journal.startup.read",
        "journal.startup.supply",
        "chat.read",
        "chat.send",
    ];
    f.post(&format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{subject}/permissions"),json!({"operation_id":"notify-console-policy","expected_change_id":null,"execution":operations.into_iter().map(|op|subject_permission(&subject,op)).collect::<Vec<_>>(),"delegation":[]})).await;
    let conversation = f
        .post(
            &conversations_path(),
            json!({"operation_id":"notify-console-conversation","subject_id":subject}),
        )
        .await;
    let c = conversation["id"].as_str().unwrap();
    let created = f.post(&format!("/api/w/{TEST_WORKSPACE_ID}/workers"),json!({"runtime_id":"connected-chat","display_name":"Notify Console","profile":"project:connected-chat","initial_submit":[],"workdir_attachments":[],"feature_connections":{"subjektiv":{"subject_id":subject}}})).await;
    let worker = created["worker_id"].as_str().unwrap();
    let chat = f
        .post(
            &messages_path(c),
            json!({"operation_id":"notify-console-chat","text":"HTTP_CHAT_RECEIPT"}),
        )
        .await;
    assert_eq!(chat["delivery"]["state"], "accepted");
    // Deliberately reuse the exact Runtime ID of the accepted HTTP chat input.
    // Its receipt must not reject/consume a different Console message.
    let canonical = serde_json::to_vec(&(
        format!(
            "account:{}",
            conversation["participant_account_id"].as_str().unwrap()
        ),
        format!("chat-message:{}", chat["message"]["id"].as_str().unwrap()),
    ))
    .unwrap();
    let digest = sha2::Sha256::digest(canonical)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let collision_id = format!("runtime-input:notify:{digest}");
    let url = format!(
        "{}/api/w/{TEST_WORKSPACE_ID}/runtimes/connected-chat/workers/{worker}/protocol/ws",
        f.url.replacen("http://", "ws://", 1)
    );
    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert(
        "authorization",
        format!("Bearer {}", f.owner).parse().unwrap(),
    );
    let (mut ws, _) = connect_async(request).await.unwrap();
    let cases = [
        (collision_id, "WS_ISOLATED_FROM_HTTP", true),
        ("a".repeat(128), "WS_ASCII_LIMIT", true),
        ("é".repeat(64), "WS_UTF8_LIMIT", true),
        ("b".repeat(129), "WS_ASCII_OVER_LIMIT", false),
        (format!("{}x", "é".repeat(64)), "WS_UTF8_OVER_LIMIT", false),
        ("console-next-id".into(), "WS_NEXT_DRAFT", true),
    ];
    let mut exchanges = Vec::new();
    for (id, text, accepted) in &cases {
        let method = protocol::Method::Notify {
            notification_request_id: id.clone(),
            message: (*text).into(),
        };
        let ack = console_notify_ack(&mut ws, &method).await;
        assert_eq!(
            matches!(ack, protocol::Event::NotificationAccepted { .. }),
            *accepted,
            "{ack:?}"
        );
        let mut delivered_ack = ack;
        if *accepted {
            // Lose the first ACK at the Console boundary, resend the exact wire
            // input, and deliver the real retry ACK to the actual tracker below.
            delivered_ack = console_notify_ack(&mut ws, &method).await;
            assert!(matches!(
                delivered_ack,
                protocol::Event::NotificationAccepted { .. }
            ));
        }
        exchanges.push(
            json!({"method":method,"event":delivered_ack,"accepted":accepted,"retry":accepted}),
        );
        if *accepted {
            let changed = protocol::Method::Notify {
                notification_request_id: id.clone(),
                message: format!("CHANGED_{text}"),
            };
            let rejected = console_notify_ack(&mut ws, &changed).await;
            assert!(
                matches!(rejected, protocol::Event::NotificationRejected { .. }),
                "{rejected:?}"
            );
            exchanges
                .push(json!({"method":changed,"event":rejected,"accepted":false,"retry":false}));
        }
    }
    let mut console = tokio::process::Command::new("deno");
    console
        .kill_on_drop(true)
        .args(["run", "--quiet"])
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/support/console_notify_admissions.ts"),
        )
        .arg(serde_json::to_string(&exchanges).unwrap());
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), console.output())
        .await
        .expect("Console admission tracker deadline")
        .expect("Deno (normal test runner prerequisite)");
    assert!(
        output.status.success(),
        "production Console admission tracker failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while !inputs
            .recv()
            .await
            .expect("real model input")
            .contains("WS_NEXT_DRAFT")
        {}
    })
    .await
    .expect("accepted Console input must reach next model request");
    let history = session_store::read_retained_session_snapshot(
        &f._dir.path().join("runtime/workers").join(worker),
        &format!("worker-runtime-{worker}"),
        2 * 1024 * 1024,
    )
    .unwrap();
    let notifications = history
        .snapshot
        .entries
        .iter()
        .filter_map(|entry| match &entry.data {
            protocol::SessionSnapshotEntryData::SystemItem {
                item_kind, content, ..
            } if item_kind == "notification" => Some(content.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    for (_, text, accepted) in cases {
        assert_eq!(
            notifications.matches(text).count(),
            usize::from(accepted),
            "one history input per accepted ID, none for rejection: {notifications}"
        );
        assert!(!notifications.contains(&format!("CHANGED_{text}")));
    }
    assert_eq!(notifications.matches("HTTP_CHAT_RECEIPT").count(), 1);
}

async fn console_notify_ack(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    method: &protocol::Method,
) -> protocol::Event {
    let protocol::Method::Notify {
        notification_request_id: expected,
        ..
    } = method
    else {
        panic!("Console Notify required")
    };
    ws.send(TungsteniteMessage::Text(
        protocol::stream::encode_method(method).unwrap().into(),
    ))
    .await
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let frame = ws.next().await.expect("real protocol WS frame").unwrap();
            if let TungsteniteMessage::Text(text) = frame {
                let event = protocol::stream::decode_event(&text).unwrap();
                match &event {
                    protocol::Event::NotificationAccepted {
                        notification_request_id,
                    }
                    | protocol::Event::NotificationRejected {
                        notification_request_id,
                        ..
                    } => {
                        assert_eq!(
                            notification_request_id, expected,
                            "client-visible ACK must preserve original request identity"
                        );
                        return event;
                    }
                    protocol::Event::Error { .. } => panic!("unexpected protocol error: {event:?}"),
                    _ => {}
                }
            }
        }
    })
    .await
    .expect("Console Notify ACK deadline")
}
