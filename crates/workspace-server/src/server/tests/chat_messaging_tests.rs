//! Real authenticated Workspace WS -> shared chat services -> FeatureStorage tests.
//! Runtime admission is Fixture's controlled boundary; explicit Subject sends still use
//! the signed current-body endpoint. Browser/model coverage is recorded separately.
use super::*;
use futures::{SinkExt, StreamExt};
use protocol::subscription::{SubscriptionFrame, SubscriptionFramePayload};
use server_api::{
    MessagingCommand, MessagingEvent, MessagingRequest, MessagingResult, MessagingSelector,
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
struct Wire {
    socket: Socket,
    pending: Vec<SubscriptionFrame>,
}
impl Wire {
    async fn connect(url: &str, token: &str) -> Self {
        let mut request = url.into_client_request().unwrap();
        request
            .headers_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
        Self {
            socket: connect_async(request).await.unwrap().0,
            pending: vec![],
        }
    }
    async fn command(&mut self, id: &str, command: MessagingCommand) {
        self.send(SubscriptionFramePayload::MessagingRequest(
            MessagingRequest {
                request_id: id.into(),
                command,
            },
        ))
        .await;
    }
    async fn send(&mut self, payload: SubscriptionFramePayload) {
        self.socket
            .send(Message::Text(
                serde_json::to_string(&SubscriptionFrame::new(payload))
                    .unwrap()
                    .into(),
            ))
            .await
            .unwrap();
    }
    async fn take(
        &mut self,
        predicate: impl Fn(&SubscriptionFramePayload) -> bool,
    ) -> SubscriptionFramePayload {
        if let Some(i) = self.pending.iter().position(|f| predicate(&f.payload)) {
            return self.pending.remove(i).payload;
        }
        loop {
            let message =
                tokio::time::timeout(std::time::Duration::from_secs(10), self.socket.next())
                    .await
                    .expect("bounded WS response")
                    .unwrap()
                    .unwrap();
            let Message::Text(text) = message else {
                continue;
            };
            let frame: SubscriptionFrame = serde_json::from_str(&text).unwrap();
            frame.validate().unwrap();
            if predicate(&frame.payload) {
                return frame.payload;
            }
            assert!(self.pending.len() < 512);
            self.pending.push(frame);
        }
    }
    async fn result(&mut self, id: &str) -> MessagingResult {
        let SubscriptionFramePayload::MessagingResponse(r) = self.take(|f| matches!(f, SubscriptionFramePayload::MessagingResponse(r) if r.request_id == id)).await else { unreachable!() };
        r.result
    }
    async fn messages(&mut self, id: &str) -> ChatMessagePage {
        let SubscriptionFramePayload::MessagingEvent(MessagingEvent::Messages { page, .. }) = self.take(|f| matches!(f, SubscriptionFramePayload::MessagingEvent(MessagingEvent::Messages { subscription_id, .. }) if subscription_id == id)).await else { unreachable!() };
        page
    }
}
async fn socket_server(f: &Fixture) -> (String, AuthorizationCallbackServer) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = build_router(f.api.clone());
    (
        format!("ws://{address}/api/w/{TEST_WORKSPACE_ID}/protocol/ws"),
        AuthorizationCallbackServer(tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        })),
    )
}
fn selector(f: &Fixture, c: &ChatConversation, after_sequence: u64) -> MessagingSelector {
    MessagingSelector {
        subject_id: f.subject.clone(),
        conversation_id: c.id.clone(),
        after_sequence,
    }
}
async fn subscribe(w: &mut Wire, s: MessagingSelector, id: &str) -> (String, ChatMessagePage) {
    w.command(
        id,
        MessagingCommand::Subscribe {
            selector: s.clone(),
        },
    )
    .await;
    let MessagingResult::Subscribed {
        subscription_id,
        selector,
        page,
    } = w.result(id).await
    else {
        panic!("subscription must succeed")
    };
    assert_eq!(selector, s);
    (subscription_id, page)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn messaging_http_and_explicit_tool_commits_share_live_pages_without_worker_log_publication()
{
    let f = Fixture::new().await;
    allow_chat(&f).await;
    let c = conversation(&f, &f.subject, "messaging-shared").await;
    let (url, _server) = socket_server(&f).await;
    let mut a = Wire::connect(&url, &f.owner).await;
    let mut b = Wire::connect(&url, &f.owner).await;
    let (a_id, snapshot) = subscribe(&mut a, selector(&f, &c, 0), "a").await;
    assert!(snapshot.messages.is_empty());
    let (b_id, _) = subscribe(&mut b, selector(&f, &c, 0), "b").await;
    let saved = user(&f, &c.id, "http-input", "public input").await;
    assert_eq!(
        a.messages(&a_id).await.messages,
        vec![saved.message.clone()]
    );
    assert_eq!(
        b.messages(&b_id).await.messages,
        vec![saved.message.clone()]
    );
    let (status, sent) = worker_chat(
        &f,
        &f.source,
        send(&c.id, "explicit-tool", Some(&saved.message.id)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{sent}");
    let a_page = a.messages(&a_id).await;
    let b_page = b.messages(&b_id).await;
    assert_eq!(a_page, b_page);
    assert_eq!(a_page.messages.len(), 1);
    assert_eq!(
        serde_json::to_value(&a_page.messages[0]).unwrap(),
        sent["message"]
    );
    assert_eq!(a_page.messages[0].text, "explicit Subject reply");
    assert_eq!(page(&f, &c.id).await.messages.len(), 2);
    a.command(
        "unsubscribe",
        MessagingCommand::Unsubscribe {
            subscription_id: a_id,
        },
    )
    .await;
    assert!(matches!(
        a.result("unsubscribe").await,
        MessagingResult::Unsubscribed { .. }
    ));
    let next = user(&f, &c.id, "next-http", "still public").await;
    assert_eq!(b.messages(&b_id).await.messages, vec![next.message]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn messaging_ack_loss_replay_conflict_and_selector_denial_leave_worker_subscription_alive() {
    use protocol::subscription::*;
    let f = Fixture::new().await;
    let c = conversation(&f, &f.subject, "messaging-replay").await;
    let (url, _server) = socket_server(&f).await;
    let mut a = Wire::connect(&url, &f.owner).await;
    a.send(SubscriptionFramePayload::Request(
        SubscriptionRequest::SubscribeEvents {
            request_id: SubscriptionRequestId::new("workers").unwrap(),
            selector: EventSubscriptionSelector::WorkspaceWorkers,
        },
    ))
    .await;
    assert!(matches!(
        a.take(|f| matches!(f, SubscriptionFramePayload::Response(_)))
            .await,
        SubscriptionFramePayload::Response(SubscriptionResponse::Subscribed { .. })
    ));
    let (id, _) = subscribe(&mut a, selector(&f, &c, 0), "chat").await;
    let request = server_api::ChatUserMessageRequest {
        operation_id: "lost-ack".into(),
        text: "save once".into(),
        reply_to: None,
        work_ref: None,
    };
    let post = MessagingCommand::Post {
        selector: selector(&f, &c, 0),
        request: request.clone(),
    };
    a.command("lost", post.clone()).await;
    let first = a.messages(&id).await.messages.remove(0); // Observe commit but lose command ACK.
    a.socket.close(None).await.unwrap();
    let mut reconnected = Wire::connect(&url, &f.owner).await;
    reconnected.command("retry", post).await;
    let MessagingResult::Saved { response } = reconnected.result("retry").await else {
        panic!("must recover saved message")
    };
    assert_eq!(response.message, first);
    let mut changed = request;
    changed.text = "different input".into();
    reconnected
        .command(
            "conflict",
            MessagingCommand::Post {
                selector: selector(&f, &c, 0),
                request: changed,
            },
        )
        .await;
    assert_eq!(
        reconnected.result("conflict").await,
        MessagingResult::Rejected { status: 409 }
    );
    let mut wrong = selector(&f, &c, 0);
    wrong.subject_id = "other-subject".into();
    reconnected
        .command(
            "wrong-selector",
            MessagingCommand::Subscribe { selector: wrong },
        )
        .await;
    assert_eq!(
        reconnected.result("wrong-selector").await,
        MessagingResult::Rejected { status: 403 }
    );
    reconnected
        .send(SubscriptionFramePayload::Request(
            SubscriptionRequest::SubscribeEvents {
                request_id: SubscriptionRequestId::new("workers-after-denial").unwrap(),
                selector: EventSubscriptionSelector::WorkspaceWorkers,
            },
        ))
        .await;
    assert!(matches!(
        reconnected
            .take(|f| matches!(f, SubscriptionFramePayload::Response(_)))
            .await,
        SubscriptionFramePayload::Response(SubscriptionResponse::Subscribed { .. })
    ));
    assert_eq!(page(&f, &c.id).await.messages, vec![first]);
    let foreign = seed_test_api_token(f.api.store.as_ref(), "messaging-foreign-account");
    let mut foreign_request = url.into_client_request().unwrap();
    foreign_request.headers_mut().insert(
        "authorization",
        format!("Bearer {foreign}").parse().unwrap(),
    );
    let Err(tokio_tungstenite::tungstenite::Error::Http(rejected)) =
        connect_async(foreign_request).await
    else {
        panic!("foreign account must not upgrade")
    };
    assert_eq!(rejected.status().as_u16(), 403);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn messaging_paginated_snapshot_race_reconnect_and_current_revocation_use_saved_sequence() {
    let f = Fixture::new().await;
    let c = conversation(&f, &f.subject, "messaging-pages").await;
    let store = open_subjektiv_store(&f.api).unwrap();
    // Storage fixture for page size, not a bypass used to claim HTTP authentication.
    for i in 0..103 {
        store
            .chat_append_user_message(
                &c.participant_account_id,
                &c.id,
                &server_api::ChatUserMessageRequest {
                    operation_id: format!("page-{i}"),
                    text: format!("message-{i}"),
                    reply_to: None,
                    work_ref: None,
                },
            )
            .unwrap();
    }
    let (url, _server) = socket_server(&f).await;
    let mut a = Wire::connect(&url, &f.owner).await;
    a.command(
        "boundary",
        MessagingCommand::Subscribe {
            selector: selector(&f, &c, 0),
        },
    )
    .await;
    let concurrent = user(&f, &c.id, "boundary-post", "during snapshot").await;
    let MessagingResult::Subscribed {
        subscription_id,
        page: first,
        ..
    } = a.result("boundary").await
    else {
        panic!("subscribed")
    };
    assert_eq!(first.messages.len(), 100);
    assert!(first.has_more);
    let mut messages = first.messages;
    while messages.len() < 104 {
        messages.extend(a.messages(&subscription_id).await.messages);
    }
    assert_eq!(
        messages,
        store
            .chat_messages_for_account(
                &c.participant_account_id,
                &c.id,
                &server_api::ChatMessagePageQuery {
                    after_sequence: None,
                    limit: Some(100)
                }
            )
            .unwrap()
            .messages
            .into_iter()
            .chain(
                store
                    .chat_messages_for_account(
                        &c.participant_account_id,
                        &c.id,
                        &server_api::ChatMessagePageQuery {
                            after_sequence: Some(messages[99].sequence),
                            limit: Some(100)
                        }
                    )
                    .unwrap()
                    .messages
            )
            .collect::<Vec<_>>()
    );
    assert_eq!(messages.last().unwrap().id, concurrent.message.id);
    let cursor = messages.last().unwrap().sequence;
    a.socket.close(None).await.unwrap();
    let offline = user(&f, &c.id, "offline", "while disconnected").await;
    let mut b = Wire::connect(&url, &f.owner).await;
    let (_, restored) = subscribe(&mut b, selector(&f, &c, cursor), "resume").await;
    assert_eq!(restored.messages, vec![offline.message]);
    f.api
        .config_store
        .with_conn(|connection| {
            connection.execute(
                "UPDATE api_tokens SET revoked_at='2026-01-02T00:00:00Z' WHERE token_hash=?1",
                [token_hash(&f.owner)],
            )?;
            Ok(())
        })
        .unwrap();
    let _ = f.api.chat_changes.send(c.id);
    // Account revocation now ends the entire Workspace connection, not only chat.
    let closed = tokio::time::timeout(std::time::Duration::from_secs(10), b.socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        matches!(closed,Message::Close(Some(frame)) if frame.code == tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Policy)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn messaging_lag_and_page_backpressure_close_only_the_bounded_subscription() {
    let f = Fixture::new().await;
    let c = conversation(&f, &f.subject, "messaging-lag").await;
    let headers = human_request(&f.owner, "GET", "/", None).headers().clone();
    let actor = resolve_actor(&ServerAuthApi::from(&f.api), &headers)
        .await
        .unwrap()
        .unwrap();
    let context = WorkerOperationContext::Browser {
        headers,
        source: authenticated_browser_input_source(&actor),
    };
    let (control, mut frames) = tokio::sync::mpsc::channel(4);
    let (data, mut pages) = tokio::sync::mpsc::channel(1);
    let mut lane = crate::workspace_messaging::MessagingLane::default();
    lane.request(
        &f.api,
        &context,
        MessagingRequest {
            request_id: "lag".into(),
            command: MessagingCommand::Subscribe {
                selector: selector(&f, &c, 0),
            },
        },
        &control,
        &data,
    )
    .await;
    // The current-thread task cannot consume until this producer has overrun the bounded hint ring.
    for _ in 0..200 {
        let _ = f.api.chat_changes.send(c.id.clone());
    }
    let receive = |message: axum::extract::ws::Message| {
        let axum::extract::ws::Message::Text(text) = message else {
            panic!("text")
        };
        serde_json::from_str::<SubscriptionFrame>(&text)
            .unwrap()
            .payload
    };
    assert!(matches!(
        receive(
            tokio::time::timeout(std::time::Duration::from_secs(5), frames.recv())
                .await
                .unwrap()
                .unwrap()
        ),
        SubscriptionFramePayload::MessagingResponse(_)
    ));
    assert!(matches!(
        receive(
            tokio::time::timeout(std::time::Duration::from_secs(5), frames.recv())
                .await
                .unwrap()
                .unwrap()
        ),
        SubscriptionFramePayload::MessagingEvent(MessagingEvent::Closed {
            reason: server_api::MessagingTermination::Lagged,
            ..
        })
    ));
    let store = open_subjektiv_store(&f.api).unwrap();
    for i in 0..210 {
        store
            .chat_append_user_message(
                &c.participant_account_id,
                &c.id,
                &server_api::ChatUserMessageRequest {
                    operation_id: format!("bounded-{i}"),
                    text: "bounded page".into(),
                    reply_to: None,
                    work_ref: None,
                },
            )
            .unwrap();
    }
    lane.request(
        &f.api,
        &context,
        MessagingRequest {
            request_id: "backpressure".into(),
            command: MessagingCommand::Subscribe {
                selector: selector(&f, &c, 0),
            },
        },
        &control,
        &data,
    )
    .await;
    let SubscriptionFramePayload::MessagingResponse(server_api::MessagingResponse {
        result: MessagingResult::Subscribed { page: snapshot, .. },
        ..
    }) = receive(
        tokio::time::timeout(std::time::Duration::from_secs(5), frames.recv())
            .await
            .unwrap()
            .unwrap(),
    )
    else {
        panic!("snapshot")
    };
    assert_eq!(snapshot.messages.len(), 100);
    assert!(matches!(
        receive(
            tokio::time::timeout(std::time::Duration::from_secs(5), frames.recv())
                .await
                .unwrap()
                .unwrap()
        ),
        SubscriptionFramePayload::MessagingEvent(MessagingEvent::Closed {
            reason: server_api::MessagingTermination::Lagged,
            ..
        })
    ));
    let SubscriptionFramePayload::MessagingEvent(MessagingEvent::Messages { page: queued, .. }) =
        receive(pages.recv().await.unwrap())
    else {
        panic!("bounded data page")
    };
    assert_eq!(queued.messages.len(), 100);
    let restored = f
        .api
        .messaging_page(
            &context,
            &selector(&f, &c, queued.messages.last().unwrap().sequence),
        )
        .await
        .unwrap();
    assert_eq!(restored.messages.len(), 10);
    assert!(!restored.has_more);
    lane.request(
        &f.api,
        &context,
        MessagingRequest {
            request_id: "still-alive".into(),
            command: MessagingCommand::Open {
                request: server_api::ChatCreateConversationRequest {
                    subject_id: f.subject.clone(),
                    operation_id: "after-lag".into(),
                },
            },
        },
        &control,
        &data,
    )
    .await;
    assert!(matches!(
        receive(
            tokio::time::timeout(std::time::Duration::from_secs(5), frames.recv())
                .await
                .unwrap()
                .unwrap()
        ),
        SubscriptionFramePayload::MessagingResponse(server_api::MessagingResponse {
            result: MessagingResult::Opened { .. },
            ..
        })
    ));
}

// Each helper is a distinct OS process with a fresh API, broadcast ring and SQLite handles.
// Only fixture admission/execution is substituted, not the HTTP/WS/storage/authentication path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "private child entry point selected only by messaging_server_process_restart"]
async fn messaging_restart_server_child() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    assert!(std::env::var_os("T855_RESTART_CHILD").is_some());
    let mut input = BufReader::new(tokio::io::stdin()).lines();
    let init: Value = serde_json::from_str(&input.next_line().await.unwrap().unwrap()).unwrap();
    let root = Path::new(init["root"].as_str().unwrap());
    assert!(root.is_absolute());
    let api = test_api(root).await;
    let (owner, conversation) = if init["bootstrap"] == true {
        let owner =
            seed_test_workspace_owner_token(api.store.as_ref(), "restart-messaging-owner").await;
        let subject = open_subjektiv_store(&api)
            .unwrap()
            .create_subject(crate::subjektiv::SubjectRole::new("restart-messaging").unwrap())
            .unwrap()
            .id;
        let (status, conversation) = account(
            &api,
            &owner,
            "POST",
            &conversations_path(),
            Some(json!({"operation_id":"restart-open","subject_id":subject})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        (owner, conversation)
    } else {
        (
            init["owner"].as_str().unwrap().to_owned(),
            init["conversation"].clone(),
        )
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopping) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(
        axum::serve(listener, build_router(api.clone()))
            .with_graceful_shutdown(async {
                let _ = stopping.await;
            })
            .into_future(),
    );
    // This IPC line is captured by the parent and never printed in validation logs.
    println!(
        "T855_SERVER_READY {}",
        json!({"url":format!("ws://{address}/api/w/{TEST_WORKSPACE_ID}/protocol/ws"),"owner":owner,"conversation":conversation})
    );
    let shutdown: Value = serde_json::from_str(&input.next_line().await.unwrap().unwrap()).unwrap();
    if shutdown["revoke"] == true {
        api.config_store
            .with_conn(|connection| {
                connection.execute(
                    "UPDATE api_tokens SET revoked_at='2026-01-02T00:00:00Z' WHERE token_hash=?1",
                    [token_hash(&owner)],
                )?;
                Ok(())
            })
            .unwrap();
    }
    stop.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    shutdown_workspace_feature_storage(&api).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn messaging_server_process_restart_recovers_committed_ack_loss_without_duplicates_and_rechecks_auth()
 {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    struct Server {
        child: tokio::process::Child,
        input: tokio::process::ChildStdin,
        state: Value,
    }
    impl Server {
        async fn start(init: Value) -> Self {
            let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "server::tests::subject_authorization_tests::chat_tests::messaging::messaging_restart_server_child", "--ignored", "--nocapture"])
                .env("T855_RESTART_CHILD", "1")
                .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null())
                .kill_on_drop(true).spawn().unwrap();
            let mut input = child.stdin.take().unwrap();
            input
                .write_all(format!("{init}\n").as_bytes())
                .await
                .unwrap();
            let mut output = BufReader::new(child.stdout.take().unwrap()).lines();
            let state = tokio::time::timeout(std::time::Duration::from_secs(30), async {
                while let Some(line) = output.next_line().await.unwrap() {
                    if let Some(wire) = line.strip_prefix("T855_SERVER_READY ") {
                        return serde_json::from_str::<Value>(wire).unwrap();
                    }
                }
                panic!("restart helper exited before readiness (credentials withheld)");
            })
            .await
            .unwrap();
            // Drain the harness's bounded tail without exposing the private readiness line.
            tokio::spawn(async move { while output.next_line().await.ok().flatten().is_some() {} });
            Self {
                child,
                input,
                state,
            }
        }
        async fn stop(mut self, revoke: bool) -> Value {
            self.input
                .write_all(format!("{}\n", json!({"revoke":revoke})).as_bytes())
                .await
                .unwrap();
            assert!(
                tokio::time::timeout(std::time::Duration::from_secs(20), self.child.wait())
                    .await
                    .unwrap()
                    .unwrap()
                    .success(),
                "restart helper failed; credentials withheld"
            );
            self.state
        }
    }
    let root = tempfile::tempdir().unwrap();
    let first = Server::start(json!({"root":root.path(),"bootstrap":true})).await;
    let c: ChatConversation = serde_json::from_value(first.state["conversation"].clone()).unwrap();
    let owner = first.state["owner"].as_str().unwrap();
    let mut socket = Wire::connect(first.state["url"].as_str().unwrap(), owner).await;
    let selector = MessagingSelector {
        subject_id: c.subject_id.clone(),
        conversation_id: c.id.clone(),
        after_sequence: 0,
    };
    let (id, initial) = subscribe(&mut socket, selector.clone(), "initial").await;
    assert!(initial.messages.is_empty());
    let command = |operation: &str| MessagingCommand::Post {
        selector: selector.clone(),
        request: server_api::ChatUserMessageRequest {
            operation_id: operation.into(),
            text: operation.into(),
            reply_to: None,
            work_ref: None,
        },
    };
    socket.command("before", command("before-shutdown")).await;
    let observed = socket.messages(&id).await.messages.remove(0);
    assert!(matches!(
        socket.result("before").await,
        MessagingResult::Saved { .. }
    ));
    socket
        .command("boundary", command("committed-before-lost-ack"))
        .await;
    let boundary = socket.messages(&id).await.messages.remove(0);
    socket.socket.close(None).await.unwrap(); // Do not read the POST ACK.
    let state = first.stop(false).await;
    let next = || json!({"root":root.path(),"bootstrap":false,"owner":state["owner"],"conversation":state["conversation"]});
    let second = Server::start(next()).await;
    let mut socket = Wire::connect(
        second.state["url"].as_str().unwrap(),
        state["owner"].as_str().unwrap(),
    )
    .await;
    let (id, recovered) = subscribe(
        &mut socket,
        MessagingSelector {
            after_sequence: observed.sequence,
            ..selector.clone()
        },
        "resume",
    )
    .await;
    assert_eq!(recovered.conversation.id, c.id);
    assert_eq!(recovered.messages, vec![boundary.clone()]);
    socket
        .command("replay", command("committed-before-lost-ack"))
        .await;
    let MessagingResult::Saved { response } = socket.result("replay").await else {
        panic!("persisted replay must recover saved result")
    };
    assert_eq!(response.message, boundary);
    socket.command("after", command("after-restart")).await;
    let after = socket.messages(&id).await.messages.remove(0);
    socket
        .command(
            "all",
            MessagingCommand::History {
                selector: selector.clone(),
                query: server_api::ChatMessagePageQuery {
                    after_sequence: Some(0),
                    limit: Some(100),
                },
            },
        )
        .await;
    let MessagingResult::History { page } = socket.result("all").await else {
        panic!("history")
    };
    assert_eq!(page.messages, vec![observed, boundary, after]);
    socket.socket.close(None).await.unwrap();
    second.stop(true).await;
    let third = Server::start(next()).await;
    let mut request = third.state["url"]
        .as_str()
        .unwrap()
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(
        "authorization",
        format!("Bearer {}", state["owner"].as_str().unwrap())
            .parse()
            .unwrap(),
    );
    let Err(tokio_tungstenite::tungstenite::Error::Http(rejected)) = connect_async(request).await
    else {
        panic!("revoked credentials must not regain a subscription after restart")
    };
    assert_eq!(rejected.status().as_u16(), 401);
    third.stop(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn messaging_subject_deletion_closes_only_its_subscription_and_wrong_workspace_never_upgrades()
 {
    use protocol::subscription::*;
    let f = Fixture::new().await;
    let subject = open_subjektiv_store(&f.api)
        .unwrap()
        .create_subject(crate::subjektiv::SubjectRole::new("delete-chat").unwrap())
        .unwrap()
        .id;
    let c = conversation(&f, &subject, "deleted-conversation").await;
    let (url, _server) = socket_server(&f).await;
    let mut wire = Wire::connect(&url, &f.owner).await;
    let (id, _) = subscribe(
        &mut wire,
        MessagingSelector {
            subject_id: subject.clone(),
            conversation_id: c.id.clone(),
            after_sequence: 0,
        },
        "deleted",
    )
    .await;
    let (status, deleted) = account(
        &f.api,
        &f.owner,
        "DELETE",
        &format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{subject}"),
        Some(json!({"operation_id":"delete-messaging-subject"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{deleted}");
    assert_eq!(deleted["completed"], true);
    let _ = f.api.chat_changes.send(c.id);
    assert!(matches!(wire.take(|frame| matches!(frame, SubscriptionFramePayload::MessagingEvent(MessagingEvent::Closed { subscription_id, .. }) if subscription_id == &id)).await, SubscriptionFramePayload::MessagingEvent(MessagingEvent::Closed { reason:server_api::MessagingTermination::AccessRevoked, .. })));
    wire.send(SubscriptionFramePayload::Request(
        SubscriptionRequest::SubscribeEvents {
            request_id: SubscriptionRequestId::new("workers-after-deletion").unwrap(),
            selector: EventSubscriptionSelector::WorkspaceWorkers,
        },
    ))
    .await;
    assert!(matches!(
        wire.take(|frame| matches!(frame, SubscriptionFramePayload::Response(_)))
            .await,
        SubscriptionFramePayload::Response(SubscriptionResponse::Subscribed { .. })
    ));
    let wrong_workspace = url.replace(TEST_WORKSPACE_ID, "missing-workspace");
    let mut request = wrong_workspace.into_client_request().unwrap();
    request.headers_mut().insert(
        "authorization",
        format!("Bearer {}", f.owner).parse().unwrap(),
    );
    let Err(tokio_tungstenite::tungstenite::Error::Http(rejected)) = connect_async(request).await
    else {
        panic!("wrong Workspace must not upgrade")
    };
    assert_eq!(rejected.status().as_u16(), 404);
}
