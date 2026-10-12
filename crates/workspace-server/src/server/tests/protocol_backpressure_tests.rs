use super::*;

const PROTOCOL_TEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

// Completion replies are produced synchronously by the actual attached execution.
// Unlike flooding the fixture's 32-entry broadcast bus, this reaches the Server's
// event admission boundary without first causing unrelated Runtime relay lag.
#[tokio::test]
async fn embedded_event_overflow_terminates_without_losing_the_admitted_prefix() {
    let (runtime, worker_ref, execution) = runtime_with_worker_backend();
    let mut connection = connect_embedded_worker_protocol(
        embedded_protocol_test_source(&runtime, &worker_ref),
        embedded_protocol_test_scope(),
    )
    .await
    .unwrap();

    // Leave the snapshot queued. Exactly capacity - 1 completion replies fit;
    // the next reply must end this connection rather than block its bridge.
    tokio::time::timeout(PROTOCOL_TEST_TIMEOUT, async {
        for index in 0..WORKER_PROTOCOL_EVENT_CAPACITY {
            connection
                .methods
                .send(protocol_test_completion(&format!("overflow-{index}")))
                .await
                .unwrap();
        }
        assert_eq!(
            connection.termination.ended().await,
            WorkerProtocolEnd::EventOverflow
        );
        connection.methods.closed().await;
    })
    .await
    .expect("full embedded event queue did not terminate the connection");

    assert_embedded_protocol_snapshot(
        next_embedded_protocol_event(&mut connection).await,
        &worker_ref,
    );
    for index in 0..WORKER_PROTOCOL_EVENT_CAPACITY - 1 {
        assert!(matches!(
            next_embedded_protocol_event(&mut connection).await,
            protocol::Event::Completions { request_id: Some(id), .. }
                if id == format!("overflow-{index}")
        ));
    }
    assert!(
        tokio::time::timeout(PROTOCOL_TEST_TIMEOUT, connection.events.recv())
            .await
            .expect("overflowed event receiver did not close")
            .is_none()
    );
    assert!(
        connection
            .methods
            .try_send(protocol_test_completion("after-overflow"))
            .is_err()
    );
    assert_eq!(
        execution.protocol_methods().len(),
        WORKER_PROTOCOL_EVENT_CAPACITY
    );
}

#[tokio::test]
async fn embedded_protocol_delivers_snapshot_then_ordered_replies_and_live_events() {
    let (runtime, worker_ref, execution) = runtime_with_worker_backend();
    let mut connection = connect_embedded_worker_protocol(
        embedded_protocol_test_source(&runtime, &worker_ref),
        embedded_protocol_test_scope(),
    )
    .await
    .unwrap();
    for id in ["first", "second", "third"] {
        connection
            .methods
            .try_send(protocol_test_completion(id))
            .unwrap();
    }
    assert_embedded_protocol_snapshot(
        next_embedded_protocol_event(&mut connection).await,
        &worker_ref,
    );
    for expected in ["first", "second", "third"] {
        assert!(matches!(
            next_embedded_protocol_event(&mut connection).await,
            protocol::Event::Completions { request_id: Some(id), .. } if id == expected
        ));
    }
    execution.emit_worker_event(
        &worker_ref,
        protocol::Event::TextDelta {
            text: "live-after-replies".into(),
        },
    );
    assert!(matches!(
        next_embedded_protocol_event(&mut connection).await,
        protocol::Event::TextDelta { text } if text == "live-after-replies"
    ));
    assert_eq!(connection.termination.reason(), None);
}

#[tokio::test]
async fn embedded_overflow_requires_explicit_reconnect_and_stop_ends_the_new_connection() {
    let (runtime, worker_ref, execution) = runtime_with_worker_backend();
    let source = embedded_protocol_test_source(&runtime, &worker_ref);
    let mut old = connect_embedded_worker_protocol(source.clone(), embedded_protocol_test_scope())
        .await
        .unwrap();
    assert_embedded_protocol_snapshot(next_embedded_protocol_event(&mut old).await, &worker_ref);
    tokio::time::timeout(PROTOCOL_TEST_TIMEOUT, async {
        for index in 0..=WORKER_PROTOCOL_EVENT_CAPACITY {
            old.methods
                .send(protocol_test_completion(&format!("old-{index}")))
                .await
                .unwrap();
        }
        assert_eq!(
            old.termination.ended().await,
            WorkerProtocolEnd::EventOverflow
        );
        old.methods.closed().await;
    })
    .await
    .expect("overflowed connection remained live");

    // Overflow ends only the attached connection, not its Worker. Reconnection
    // is explicit and starts from a fresh execution snapshot, not old backlog.
    let mut fresh = connect_embedded_worker_protocol(source, embedded_protocol_test_scope())
        .await
        .unwrap();
    assert_embedded_protocol_snapshot(next_embedded_protocol_event(&mut fresh).await, &worker_ref);
    fresh
        .methods
        .try_send(protocol_test_completion("explicit-reconnect"))
        .unwrap();
    assert!(matches!(
        next_embedded_protocol_event(&mut fresh).await,
        protocol::Event::Completions { request_id: Some(id), .. } if id == "explicit-reconnect"
    ));
    assert!(
        old.methods
            .try_send(protocol_test_completion("must-not-reattach"))
            .is_err()
    );
    assert_eq!(
        old.termination.reason(),
        Some(WorkerProtocolEnd::EventOverflow)
    );
    assert_eq!(
        execution.protocol_methods().len(),
        WORKER_PROTOCOL_EVENT_CAPACITY + 2
    );

    runtime.stop_worker(&worker_ref, None).unwrap();
    assert_eq!(
        tokio::time::timeout(PROTOCOL_TEST_TIMEOUT, fresh.termination.ended())
            .await
            .expect("Stop did not end the attached protocol connection"),
        WorkerProtocolEnd::Disconnected
    );
    assert_embedded_protocol_closed(&mut fresh).await;
}

async fn backpressure_browser_context(api: &WorkspaceApi) -> (WorkerOperationContext, String) {
    seed_test_api_token(api.store.as_ref(), "backpressure-browser");
    let token = "backpressure-browser-session";
    let token_hash = crate::auth::token_hash(token);
    api.store
        .create_browser_session(&BrowserSessionRecord {
            token_hash: token_hash.clone(),
            session_id: "backpressure-session".into(),
            user_id: "user-backpressure-browser".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            expires_at: "2099-01-01T00:00:00Z".into(),
            revoked_at: None,
        })
        .unwrap();
    let AuthConfig::Passkey {
        cookie_name,
        origin,
        ..
    } = &api.config.auth;
    let mut headers = HeaderMap::new();
    headers.insert(ORIGIN, origin.parse().unwrap());
    headers.insert(
        axum::http::header::COOKIE,
        format!("{cookie_name}={token}").parse().unwrap(),
    );
    let actor = resolve_actor(&ServerAuthApi::from(api), &headers)
        .await
        .unwrap()
        .expect("fixture browser session must authenticate");
    (
        WorkerOperationContext::Browser {
            headers,
            source: authenticated_browser_input_source(&actor),
        },
        token_hash,
    )
}

async fn undrained_protocol_connection() -> (
    WorkspaceWorkerProtocolConnection,
    tokio::sync::mpsc::Receiver<protocol::Method>,
) {
    let (receiver_sender, receiver) = tokio::sync::oneshot::channel();
    let connection = spawn_worker_protocol_pump(move |methods, events| async move {
        receiver_sender.send(methods).unwrap();
        // Keep the event endpoint live until common termination cancels this
        // fixture pump. Method consumption is controlled solely by the test.
        let _events = events;
        std::future::pending::<WorkerProtocolEnd>().await
    });
    let receiver = tokio::time::timeout(PROTOCOL_TEST_TIMEOUT, receiver)
        .await
        .expect("fixture pump did not expose its undrained method endpoint")
        .unwrap();
    (connection, receiver)
}

async fn full_protocol_method_queue() -> (
    WorkspaceWorkerProtocolConnection,
    tokio::sync::mpsc::Receiver<protocol::Method>,
) {
    let (connection, receiver) = undrained_protocol_connection().await;
    for index in 0..WORKER_PROTOCOL_METHOD_CAPACITY {
        connection
            .methods
            .try_send(protocol_test_completion(&format!("queued-{index}")))
            .unwrap();
    }
    (connection, receiver)
}

#[tokio::test]
async fn browser_method_queue_overflow_fails_fast_and_does_not_pin_session_cleanup() {
    let workspace = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(workspace.path()).await;
    let identity = spawn_ticket_check_source(&api, "method-overflow");
    let worker = WorkspaceWorker::resolve(&api, &identity.runtime_id, &identity.worker_id).unwrap();
    let (context, _) = backpressure_browser_context(&api).await;
    let (connection, mut receiver) = full_protocol_method_queue().await;
    let termination = connection.termination.clone();
    let sender =
        WorkspaceWorkerMethodSender::new(worker.clone(), connection.methods, termination.clone());
    let _events = connection.events;

    let result = tokio::time::timeout(
        PROTOCOL_TEST_TIMEOUT,
        sender.send(
            &context,
            protocol::Method::Cancel {
                command: protocol::WorkerCommandEnvelope::new(801),
            },
        ),
    )
    .await
    .expect("method admission blocked while holding the session guard");
    assert!(result.is_err(), "full method queue must reject Cancel too");
    assert_eq!(
        termination.reason(),
        Some(WorkerProtocolEnd::MethodOverflow)
    );
    assert_eq!(receiver.len(), WORKER_PROTOCOL_METHOD_CAPACITY);
    assert!(matches!(
        receiver.try_recv().unwrap(),
        protocol::Method::ListCompletions { request_id: Some(id), .. } if id == "queued-0"
    ));
    assert!(
        sender
            .send(
                &context,
                protocol_test_completion("must-not-retry-after-drain")
            )
            .await
            .is_err()
    );
    assert_eq!(receiver.len(), WORKER_PROTOCOL_METHOD_CAPACITY - 1);

    let stopped = tokio::time::timeout(
        PROTOCOL_TEST_TIMEOUT,
        worker.stop(
            &WorkerOperationContext::Backend,
            WorkerLifecycleRequest {
                reason: Some("cleanup-after-method-overflow".into()),
                ticket_assignment: None,
            },
        ),
    )
    .await
    .expect("full method queue pinned Stop's session cleanup")
    .unwrap();
    assert_eq!(stopped.state, InternalWorkerOperationState::Accepted);
    assert!(execution.protocol_methods().is_empty());
    for index in 1..WORKER_PROTOCOL_METHOD_CAPACITY {
        assert!(matches!(
            receiver.try_recv().unwrap(),
            protocol::Method::ListCompletions { request_id: Some(id), .. }
                if id == format!("queued-{index}")
        ));
    }
    assert!(receiver.try_recv().is_err());
}

#[tokio::test]
async fn revoked_browser_credentials_reject_before_method_overflow_admission() {
    let workspace = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(workspace.path()).await;
    let identity = spawn_ticket_check_source(&api, "revoked-full-queue");
    let worker = WorkspaceWorker::resolve(&api, &identity.runtime_id, &identity.worker_id).unwrap();
    let (context, token_hash) = backpressure_browser_context(&api).await;
    let (connection, receiver) = full_protocol_method_queue().await;
    let termination = connection.termination.clone();
    let sender = WorkspaceWorkerMethodSender::new(worker, connection.methods, termination.clone());
    let _events = connection.events;
    assert!(
        api.store
            .revoke_browser_session(&token_hash, &now_registry_timestamp())
            .unwrap()
    );

    let result = tokio::time::timeout(
        PROTOCOL_TEST_TIMEOUT,
        sender.send(&context, protocol_test_completion("unauthorized")),
    )
    .await
    .expect("revoked credentials waited for method capacity");
    assert!(matches!(result, Err(Error::WorkspacePermissionDenied(_))));
    assert_eq!(termination.reason(), None);
    assert_eq!(receiver.len(), WORKER_PROTOCOL_METHOD_CAPACITY);
    assert!(execution.protocol_methods().is_empty());
}

#[tokio::test]
async fn cancelling_pending_browser_admission_does_not_enqueue_or_hold_the_session_guard() {
    let workspace = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(workspace.path()).await;
    let identity = spawn_ticket_check_source(&api, "cancel-pending-admission");
    let worker = WorkspaceWorker::resolve(&api, &identity.runtime_id, &identity.worker_id).unwrap();
    let (context, _) = backpressure_browser_context(&api).await;
    let (connection, mut receiver) = undrained_protocol_connection().await;
    let termination = connection.termination.clone();
    let sender = WorkspaceWorkerMethodSender::new(worker, connection.methods, termination.clone());
    let _events = connection.events;
    let session_lock = current_worker_session_lock(&api, &identity);
    let guard = session_lock.lock().await;
    {
        let admission = sender.send(&context, protocol_test_completion("cancelled-admission"));
        tokio::pin!(admission);
        assert!(futures::poll!(admission.as_mut()).is_pending());
        // Dropping a blocked admission future must not schedule a detached retry.
    }
    drop(guard);
    assert!(receiver.try_recv().is_err());
    let guard = tokio::time::timeout(PROTOCOL_TEST_TIMEOUT, session_lock.lock())
        .await
        .expect("cancelled admission retained the session guard");
    close_current_worker_session_locked(&api, &identity)
        .await
        .unwrap();
    drop(guard);

    let command = protocol::WorkerCommandEnvelope::new(802);
    sender
        .send(&context, protocol::Method::Cancel { command })
        .await
        .unwrap();
    assert!(matches!(
        receiver.try_recv().unwrap(),
        protocol::Method::Cancel { command: admitted } if admitted == command
    ));
    assert!(receiver.try_recv().is_err());
    assert_eq!(termination.reason(), None);
    assert!(execution.protocol_methods().is_empty());
}

// In-process browser transport: one real JSON method frame, then an idle
// connection. Its writes are immediately ready, isolating authorization waits
// from transport congestion and without crossing a complete product flow.
struct AdmissionCancellationSocket {
    inbound: Option<WsMessage>,
    written: Vec<WsMessage>,
}

impl futures::Stream for AdmissionCancellationSocket {
    type Item = std::result::Result<WsMessage, axum::Error>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        match self.inbound.take() {
            Some(message) => std::task::Poll::Ready(Some(Ok(message))),
            None => std::task::Poll::Pending,
        }
    }
}

impl futures::Sink<WsMessage> for AdmissionCancellationSocket {
    type Error = std::convert::Infallible;

    fn poll_ready(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::result::Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn start_send(
        mut self: std::pin::Pin<&mut Self>,
        message: WsMessage,
    ) -> std::result::Result<(), Self::Error> {
        self.written.push(message);
        Ok(())
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::result::Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_close(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::result::Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn browser_protocol_termination_cancels_admission_waiting_for_session_authority() {
    let workspace = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(workspace.path()).await;
    let identity = spawn_ticket_check_source(&api, "browser-authorization-cancellation");
    let worker = WorkspaceWorker::resolve(&api, &identity.runtime_id, &identity.worker_id).unwrap();
    let (context, _) = backpressure_browser_context(&api).await;
    let (connection, mut receiver) = undrained_protocol_connection().await;
    let termination = connection.termination.clone();
    let sender = WorkspaceWorkerMethodSender::new(worker, connection.methods, termination.clone());
    let connection = worker_operations::WorkspaceWorkerConnection {
        sender,
        events: connection.events,
    };
    let session_lock = current_worker_session_lock(&api, &identity);
    let guard = session_lock.lock().await;
    let method = protocol::Method::Cancel {
        command: protocol::WorkerCommandEnvelope::new(803),
    };
    let mut socket = AdmissionCancellationSocket {
        inbound: Some(WsMessage::Text(
            protocol::stream::encode_method(&method).unwrap().into(),
        )),
        written: Vec::new(),
    };
    {
        let session = workspace_worker_protocol_session(connection, &mut socket, context);
        tokio::pin!(session);
        // SQLite browser authorization is synchronous; this poll consumes the
        // frame and reaches the deliberately held session lock.
        assert!(futures::poll!(session.as_mut()).is_pending());
        termination.terminate(WorkerProtocolEnd::Disconnected);
        tokio::time::timeout(PROTOCOL_TEST_TIMEOUT, session)
            .await
            .expect("terminal connection did not cancel pending browser admission");
    }
    assert!(matches!(socket.written.last(), Some(WsMessage::Close(_))));
    assert!(receiver.try_recv().is_err());
    drop(guard);
    let _guard = tokio::time::timeout(PROTOCOL_TEST_TIMEOUT, session_lock.lock())
        .await
        .expect("cancelled browser authorization pinned session cleanup");
    assert!(execution.protocol_methods().is_empty());
}

struct BackpressureTestServer(tokio::task::JoinHandle<()>);

impl Drop for BackpressureTestServer {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[tokio::test]
async fn browser_protocol_session_ends_on_full_method_queue_without_blocking_cleanup() {
    let workspace = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(workspace.path()).await;
    let identity = spawn_ticket_check_source(&api, "browser-session-overflow");
    let worker = WorkspaceWorker::resolve(&api, &identity.runtime_id, &identity.worker_id).unwrap();
    let (context, _) = backpressure_browser_context(&api).await;
    let (connection, receiver) = full_protocol_method_queue().await;
    let termination = connection.termination.clone();
    let sender = WorkspaceWorkerMethodSender::new(worker, connection.methods, termination.clone());
    let events = connection.events;
    let (session_done, finished) = tokio::sync::oneshot::channel();
    let pending = Arc::new(std::sync::Mutex::new(Some((
        worker_operations::WorkspaceWorkerConnection { sender, events },
        session_done,
    ))));
    let app = Router::new().route(
        "/protocol/ws",
        get(move |ws: WebSocketUpgrade| {
            let pending = pending.clone();
            let context = context.clone();
            async move {
                ws.on_upgrade(move |socket| async move {
                    let (connection, session_done) = pending.lock().unwrap().take().unwrap();
                    workspace_worker_protocol_session(connection, socket, context).await;
                    let _ = session_done.send(());
                })
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let _server = BackpressureTestServer(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    let (mut socket, _) = connect_async(format!("ws://{address}/protocol/ws"))
        .await
        .unwrap();
    tokio::time::timeout(
        PROTOCOL_TEST_TIMEOUT,
        send_protocol_test_ws_method(&mut socket, &protocol_test_completion("browser-overflow")),
    )
    .await
    .expect("browser method frame did not reach the loopback transport");
    assert_eq!(
        tokio::time::timeout(PROTOCOL_TEST_TIMEOUT, termination.ended())
            .await
            .expect("browser session did not report method overflow"),
        WorkerProtocolEnd::MethodOverflow
    );
    tokio::time::timeout(PROTOCOL_TEST_TIMEOUT, finished)
        .await
        .expect("browser protocol session did not finish after overflow")
        .expect("browser protocol session dropped its completion signal");
    let session_lock = current_worker_session_lock(&api, &identity);
    let _guard = tokio::time::timeout(PROTOCOL_TEST_TIMEOUT, session_lock.lock())
        .await
        .expect("browser method overflow pinned session cleanup");
    close_current_worker_session_locked(&api, &identity)
        .await
        .unwrap();
    assert_eq!(receiver.len(), WORKER_PROTOCOL_METHOD_CAPACITY);
    assert!(execution.protocol_methods().is_empty());
}

// Accepting an output frame is not proof of delivery: this browser transport
// never finishes flushing, including the final best-effort Close frame.
struct StalledBrowserOutput {
    entered: Option<tokio::sync::oneshot::Sender<()>>,
    dropped: Option<tokio::sync::oneshot::Sender<()>>,
    written: Arc<std::sync::Mutex<Vec<WsMessage>>>,
}

impl futures::Stream for StalledBrowserOutput {
    type Item = std::result::Result<WsMessage, axum::Error>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::task::Poll::Pending
    }
}

impl futures::Sink<WsMessage> for StalledBrowserOutput {
    type Error = std::convert::Infallible;

    fn poll_ready(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::result::Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn start_send(
        mut self: std::pin::Pin<&mut Self>,
        message: WsMessage,
    ) -> std::result::Result<(), Self::Error> {
        self.written.lock().unwrap().push(message);
        if let Some(entered) = self.entered.take() {
            let _ = entered.send(());
        }
        Ok(())
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::result::Result<(), Self::Error>> {
        std::task::Poll::Pending
    }

    fn poll_close(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::result::Result<(), Self::Error>> {
        std::task::Poll::Pending
    }
}

impl Drop for StalledBrowserOutput {
    fn drop(&mut self) {
        if let Some(dropped) = self.dropped.take() {
            let _ = dropped.send(());
        }
    }
}

fn assert_bounded_unknown_delivery_close(messages: &[WsMessage]) {
    let Some(WsMessage::Close(Some(frame))) = messages.last() else {
        panic!("terminal browser session must attempt an explicit Close frame: {messages:?}");
    };
    assert_eq!(frame.code, 1011);
    assert!(frame.reason.contains("delivery unknown"), "{frame:?}");
    assert!(frame.reason.contains("no retry"), "{frame:?}");
    assert!(
        frame.reason.len() < 123,
        "Close reason exceeds its wire bound: {frame:?}"
    );
}

async fn assert_same_worker_reconnect_and_stop(
    worker: &WorkspaceWorker,
    context: &WorkerOperationContext,
) {
    let mut fresh = tokio::time::timeout(PROTOCOL_TEST_TIMEOUT, worker.connect_protocol(context))
        .await
        .expect("terminated transport retained same Worker's connection authority")
        .unwrap();
    assert!(matches!(
        tokio::time::timeout(PROTOCOL_TEST_TIMEOUT, fresh.events.recv())
            .await
            .expect("same Worker reconnect did not deliver a fresh snapshot"),
        Some(protocol::Event::Snapshot { .. })
    ));
    let stopped = tokio::time::timeout(
        PROTOCOL_TEST_TIMEOUT,
        worker.stop(
            &WorkerOperationContext::Backend,
            WorkerLifecycleRequest {
                reason: Some("cleanup-after-stalled-protocol".into()),
                ticket_assignment: None,
            },
        ),
    )
    .await
    .expect("terminated transport pinned same Worker's Stop/session cleanup")
    .unwrap();
    assert_eq!(stopped.state, InternalWorkerOperationState::Accepted);
    assert_eq!(
        tokio::time::timeout(PROTOCOL_TEST_TIMEOUT, fresh.sender.termination.ended())
            .await
            .expect("Stop did not terminate the fresh same Worker connection"),
        WorkerProtocolEnd::Disconnected
    );
}

async fn assert_stalled_browser_output_reclaimed(explicit_termination: bool) {
    let workspace = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(workspace.path()).await;
    let identity = spawn_ticket_check_source(&api, "stalled-browser-output");
    let worker = WorkspaceWorker::resolve(&api, &identity.runtime_id, &identity.worker_id).unwrap();
    let (context, _) = backpressure_browser_context(&api).await;
    // Use the real attached embedded execution snapshot as browser output.
    let raw = connect_workspace_worker_protocol(&api, &identity, None)
        .await
        .unwrap();
    let methods = raw.methods.clone();
    let termination = raw.termination;
    let connection = worker_operations::WorkspaceWorkerConnection {
        sender: WorkspaceWorkerMethodSender::new(worker.clone(), raw.methods, termination.clone()),
        events: raw.events,
    };
    let (entered_sender, entered) = tokio::sync::oneshot::channel();
    let (dropped_sender, dropped) = tokio::sync::oneshot::channel();
    let written = Arc::new(std::sync::Mutex::new(Vec::new()));
    let socket = StalledBrowserOutput {
        entered: Some(entered_sender),
        dropped: Some(dropped_sender),
        written: written.clone(),
    };
    let started = tokio::time::Instant::now();
    {
        let session = workspace_worker_protocol_session(connection, socket, context.clone());
        tokio::pin!(session);
        assert!(futures::poll!(session.as_mut()).is_pending());
        // Poll the real session alongside its output-acceptance oneshot rather
        // than assuming how many scheduler turns the attached pump needs.
        let output_accepted = async {
            tokio::select! {
                result = entered => result.expect("browser sink dropped before receiving output"),
                _ = session.as_mut() => panic!("browser session ended before accepting output"),
            }
        };
        tokio::time::timeout(PROTOCOL_TEST_TIMEOUT, output_accepted)
            .await
            .expect("attached snapshot never reached the stalled browser output");
        {
            let messages = written.lock().unwrap();
            assert_eq!(messages.len(), 1);
            let WsMessage::Text(text) = &messages[0] else {
                panic!("first browser output must be the attached snapshot");
            };
            assert!(matches!(
                protocol::stream::decode_event(text.as_str()).unwrap(),
                protocol::Event::Snapshot { .. }
            ));
        }

        if explicit_termination {
            termination.terminate(WorkerProtocolEnd::Disconnected);
        } else {
            tokio::time::advance(std::time::Duration::from_secs(9)).await;
            assert!(futures::poll!(session.as_mut()).is_pending());
            assert_eq!(termination.reason(), None);
            tokio::time::advance(std::time::Duration::from_secs(1)).await;
        }
        assert!(futures::poll!(session.as_mut()).is_pending());
        assert_eq!(
            termination.reason(),
            Some(if explicit_termination {
                WorkerProtocolEnd::Disconnected
            } else {
                WorkerProtocolEnd::TransportTimeout
            }),
            "browser end must publish the concrete common termination cause"
        );
        {
            let messages = written.lock().unwrap();
            assert_eq!(messages.len(), 2, "must not retry uncertain browser output");
            assert_bounded_unknown_delivery_close(&messages);
        }
        tokio::time::timeout(PROTOCOL_TEST_TIMEOUT, methods.closed())
            .await
            .expect("browser termination did not cancel its actual attached pump");
        let session_lock = current_worker_session_lock(&api, &identity);
        let guard = tokio::time::timeout(PROTOCOL_TEST_TIMEOUT, session_lock.lock())
            .await
            .expect("blocked browser Close pinned session ownership");
        close_current_worker_session_locked(&api, &identity)
            .await
            .unwrap();
        drop(guard);

        // The terminal Close itself is stalled. It must get a separate bounded
        // attempt, not become a second unbounded output wait.
        tokio::time::advance(std::time::Duration::from_secs(9)).await;
        assert!(futures::poll!(session.as_mut()).is_pending());
        tokio::time::advance(std::time::Duration::from_secs(1)).await;
        tokio::time::timeout(PROTOCOL_TEST_TIMEOUT, session.as_mut())
            .await
            .expect("stalled terminal Close did not reach its deadline");
    }
    tokio::time::timeout(PROTOCOL_TEST_TIMEOUT, dropped)
        .await
        .expect("completed browser session retained its stalled sink")
        .expect("browser sink dropped without its ownership signal");
    assert_eq!(written.lock().unwrap().len(), 2);
    assert_eq!(
        tokio::time::Instant::now() - started,
        std::time::Duration::from_secs(if explicit_termination { 10 } else { 20 })
    );
    assert!(execution.protocol_methods().is_empty());
    assert_same_worker_reconnect_and_stop(&worker, &context).await;
}

#[tokio::test(start_paused = true)]
async fn browser_blocked_output_deadline_ends_pump_and_bounds_the_terminal_close() {
    assert_stalled_browser_output_reclaimed(false).await;
}

#[tokio::test(start_paused = true)]
async fn browser_common_termination_preempts_blocked_output_and_bounds_the_terminal_close() {
    assert_stalled_browser_output_reclaimed(true).await;
}

#[tokio::test]
async fn embedded_disconnect_and_last_owner_drop_release_the_execution_event_subscription() {
    for explicit_termination in [false, true] {
        let (runtime, worker_ref, execution) = runtime_with_worker_backend();
        let endpoint = execution
            .contexts
            .lock()
            .unwrap()
            .get(&worker_ref)
            .cloned()
            .unwrap();
        let mut connection = connect_embedded_worker_protocol(
            embedded_protocol_test_source(&runtime, &worker_ref),
            embedded_protocol_test_scope(),
        )
        .await
        .unwrap();
        assert_embedded_protocol_snapshot(
            next_embedded_protocol_event(&mut connection).await,
            &worker_ref,
        );
        assert_eq!(endpoint.events.receiver_count(), 1);
        let WorkspaceWorkerProtocolConnection {
            methods,
            mut events,
            termination,
        } = connection;
        let last_owner = termination.clone();
        drop(termination);
        assert!(!methods.is_closed());
        if explicit_termination {
            last_owner.terminate(WorkerProtocolEnd::Disconnected);
            // Keep the owner alive until watch cancellation is observed so
            // abort-on-drop cannot hide a broken explicit-disconnect path.
            tokio::time::timeout(PROTOCOL_TEST_TIMEOUT, methods.closed())
                .await
                .expect("embedded explicit disconnect did not reclaim its pump");
        }
        // Both channel endpoints remain held. Without explicit termination,
        // only dropping the last client owner can cancel this attached pump.
        drop(last_owner);
        tokio::time::timeout(PROTOCOL_TEST_TIMEOUT, methods.closed())
            .await
            .expect("embedded last-owner drop did not reclaim its method pump");
        assert!(
            tokio::time::timeout(PROTOCOL_TEST_TIMEOUT, events.recv())
                .await
                .expect("reclaimed embedded event endpoint stayed open")
                .is_none()
        );
        // The fixture's execution relay notices its dropped receiver on its
        // next event. Wake that relay once, then observe subscription closure.
        let _ = endpoint.events.send(protocol::Event::TextDelta {
            text: "execution-relay-close-probe".into(),
        });
        tokio::time::timeout(PROTOCOL_TEST_TIMEOUT, endpoint.events.closed())
            .await
            .expect("dropped embedded transport retained its execution event subscription");
        assert_eq!(endpoint.events.receiver_count(), 0);
        assert!(execution.protocol_methods().is_empty());
        runtime.stop_worker(&worker_ref, None).unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn stalled_remote_bridge_browser_cancel_overflow_reclaims_wire_and_allows_same_worker_progress()
 {
    let workspace = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(workspace.path()).await;
    let identity = spawn_ticket_check_source(&api, "remote-browser-overflow");
    let worker = WorkspaceWorker::resolve(&api, &identity.runtime_id, &identity.worker_id).unwrap();
    let (context, _) = backpressure_browser_context(&api).await;
    let (raw, entered, dropped, sent) = super::worker_protocol_tests::stalled_remote_bridge();
    raw.methods
        .try_send(protocol_test_completion("wire-delivery-unknown"))
        .unwrap();
    tokio::time::timeout(PROTOCOL_TEST_TIMEOUT, entered)
        .await
        .expect("actual remote pump did not enter its stalled wire send")
        .unwrap();
    for index in 0..WORKER_PROTOCOL_METHOD_CAPACITY {
        raw.methods
            .try_send(protocol_test_completion(&format!("remote-queued-{index}")))
            .unwrap();
    }
    let methods = raw.methods.clone();
    let termination = raw.termination;
    let connection = worker_operations::WorkspaceWorkerConnection {
        sender: WorkspaceWorkerMethodSender::new(worker.clone(), raw.methods, termination.clone()),
        events: raw.events,
    };
    let cancel = protocol::Method::Cancel {
        command: protocol::WorkerCommandEnvelope::new(804),
    };
    let mut socket = AdmissionCancellationSocket {
        inbound: Some(WsMessage::Text(
            protocol::stream::encode_method(&cancel).unwrap().into(),
        )),
        written: Vec::new(),
    };
    // This real browser session decodes Cancel and passes the authenticated
    // context through the actual method sender into the saturated remote pump.
    tokio::time::timeout(
        PROTOCOL_TEST_TIMEOUT,
        workspace_worker_protocol_session(connection, &mut socket, context.clone()),
    )
    .await
    .expect("browser Cancel waited for the stalled remote wire instead of failing admission");
    assert_eq!(
        termination.reason(),
        Some(WorkerProtocolEnd::MethodOverflow)
    );
    assert!(socket.inbound.is_none());
    assert_bounded_unknown_delivery_close(&socket.written);
    tokio::time::timeout(PROTOCOL_TEST_TIMEOUT, dropped)
        .await
        .expect("method overflow did not drop the stalled remote sink")
        .unwrap();
    assert!(methods.is_closed());
    {
        let messages = sent.lock().unwrap();
        assert_eq!(
            messages.len(),
            1,
            "must not retry or drain unknown remote delivery"
        );
        let TungsteniteMessage::Text(text) = &messages[0] else {
            panic!("remote pump must send an actual encoded method frame");
        };
        assert!(matches!(
            decode_method(text.as_str()).unwrap(),
            protocol::Method::ListCompletions { request_id: Some(id), .. }
                if id == "wire-delivery-unknown"
        ));
    }
    assert!(execution.protocol_methods().is_empty());
    assert_same_worker_reconnect_and_stop(&worker, &context).await;
}
