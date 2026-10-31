//! Connected Subject Body API regressions in the normal Server lib target
//! (`subject_body_connected` filter). The authenticated human router, signed
//! Backend <-> RuntimeHTTP requests, ProfileRuntimeWorkerFactory/Controller,
//! singleton reservations, SQLite and canonical FS Session storage are real.
//! Only the external model stream is scripted. One transport fault discards an
//! already-produced stop response; it does not replace stop execution or receipts.
//! Existing Memory is fixture input through the domain store; Journal admission
//! uses real, authorized canonical Session capture, not invented source identity.
//!
//! Restore requires the runner-owned isolation checked by
//! `tests/support/restore_environment.rs` before any fixture starts. This module
//! never sets process environment, installs tools, or contacts a live model.
//! Resource races and Workdir rebinding belong to the separate lifecycle suite.
use super::subject_body_tests::assert_failed_request;
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const DEADLINE: Duration = Duration::from_secs(30);

#[derive(Clone)]
struct BodyModel(tokio::sync::mpsc::Sender<String>);

#[async_trait::async_trait]
impl agen::llm_client::LlmClient for BodyModel {
    fn clone_boxed(&self) -> Box<dyn agen::llm_client::LlmClient> {
        Box::new(self.clone())
    }

    async fn stream(
        &self,
        request: agen::llm_client::Request,
    ) -> std::result::Result<agen::llm_client::client::ResponseStream, agen::llm_client::ClientError>
    {
        use agen::llm_client::event::{Event, StopReason};
        for name in [
            "ConversationRead",
            "ConversationSend",
            "SubjektivMemoryQuery",
        ] {
            assert!(
                request.tools.iter().any(|tool| tool.name == name),
                "the unchanged Subject conversation profile must expose {name}"
            );
        }
        self.0.send(format!("{:?}", request.items)).await.unwrap();
        Ok(Box::pin(futures::stream::iter([
            Ok(Event::text_block_start(0)),
            Ok(Event::text_delta(0, "Retained internal body turn")),
            Ok(Event::text_block_stop(0, Some(StopReason::EndTurn))),
        ])))
    }
}

struct ConnectedBody {
    connected: ConnectedWorkerHarness,
    lose_stop_ack: Arc<AtomicBool>,
    inputs: tokio::sync::mpsc::Receiver<String>,
}

impl ConnectedBody {
    async fn new() -> Self {
        // Fail before starting either server or touching restore allocation paths.
        crate::restore_test_environment::require_restore_test_environment();
        let (tx, inputs) = tokio::sync::mpsc::channel(8);
        let lose_stop_ack = Arc::new(AtomicBool::new(false));
        let fault = lose_stop_ack.clone();
        let connected = ConnectedWorkerHarness::start_with_layer(BodyModel(tx), move |router| {
            router.layer(axum::middleware::from_fn(
                move |request: axum::extract::Request, next: axum::middleware::Next| {
                    let fault = fault.clone();
                    async move {
                        let discard = request.uri().path().ends_with("/body/stop")
                            && fault.swap(false, Ordering::SeqCst);
                        let result = next.run(request).await;
                        if discard {
                            assert!(
                                result.status().is_success(),
                                "stop must execute before ACK loss"
                            );
                            return Response::builder()
                                .status(StatusCode::BAD_GATEWAY)
                                .header(CONTENT_TYPE, "application/json")
                                .body(Body::from(
                                    json!({"message":"injected lost stop response"}).to_string(),
                                ))
                                .unwrap();
                        }
                        result
                    }
                },
            ))
        })
        .await;
        // Body start intentionally chooses a Runtime itself and prefers embedded.
        // Remove only the unused deterministic fixture Runtime via its real idle
        // removal contract; never route around signed HTTP or replace execution.
        assert!(matches!(
            connected
                .api
                .runtime
                .unregister_if_idle(EMBEDDED_WORKER_RUNTIME_ID, 100)
                .unwrap(),
            crate::hosts::RuntimeRegistryUnregisterResult::Removed
        ));
        let available = connected.api.runtime.list_runtimes(100);
        assert_eq!(available.items.len(), 1);
        assert_eq!(available.items[0].runtime_id, CONNECTED_RUNTIME);
        assert!(available.items[0].worker_creation_available);
        Self {
            connected,
            lose_stop_ack,
            inputs,
        }
    }

    async fn request(&self, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
        self.connected
            .human(method.parse().unwrap(), path, body)
            .await
    }

    async fn ok(&self, method: &str, path: &str, body: Value) -> Value {
        let (status, result) = self.request(method, path, body).await;
        assert_eq!(status, StatusCode::OK, "{method} {path}: {result}");
        result
    }

    async fn subject(&self, operation: &str) -> String {
        let (status, created) = self
            .request(
                "POST",
                &format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects"),
                json!({"operation_id":operation,"role":"connected-body","behavior_md":""}),
            )
            .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "normal Subject creation: {created}"
        );
        created["id"].as_str().unwrap().to_owned()
    }

    async fn read(&self, subject: &str) -> Value {
        self.ok("GET", &body_path(subject), Value::Null).await
    }

    async fn mutate(&self, subject: &str, suffix: &str, request: Value) -> Value {
        self.ok("POST", &format!("{}{suffix}", body_path(subject)), request)
            .await
    }

    async fn stop(&self, subject: &str, request_id: &str) -> Value {
        let current = self.read(subject).await;
        let request = intent(request_id, &current);
        let stopped = self.mutate(subject, "/stop", request.clone()).await;
        assert_eq!(stopped["disposition"], "accepted", "{stopped}");
        let outcome = self.mutate(subject, "/stop/outcome", request.clone()).await;
        assert_eq!(outcome["outcome"], "stopped", "{outcome}");
        request
    }

    fn worker_count(&self) -> usize {
        self.connected
            .api
            .runtime
            .list_workers_for_runtime(CONNECTED_RUNTIME, 100)
            .unwrap()
            .items
            .len()
    }
}

fn body_path(subject: &str) -> String {
    format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{subject}/body")
}

fn absent_start(request_id: &str) -> Value {
    json!({"request_id":request_id,"expected_current":null,"expected_observation_token":null})
}

fn intent(request_id: &str, body: &Value) -> Value {
    let token = body["observation"]["worker"]["restore_observation_token"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("available body must expose the Runtime observation fence: {body}")
        });
    json!({"request_id":request_id,"expected_current":body["current"],"expected_observation_token":token})
}

async fn body_idle(
    events: &mut tokio::sync::broadcast::Receiver<
        worker_runtime::observation::WorkerObservationEvent,
    >,
    worker: &str,
) {
    tokio::time::timeout(DEADLINE, async {
        loop {
            let event = events.recv().await.expect("real Runtime observation");
            if event.worker_ref.worker_id.to_string() != worker {
                continue;
            }
            match event.payload {
                protocol::Event::Error { code, message } => {
                    panic!("body turn failed: {code:?}: {message}")
                }
                protocol::Event::WorkerState { snapshot }
                    if snapshot.state == protocol::WorkerState::Idle =>
                {
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("Controller must publish Idle after the scripted model turn");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn body_same_request_concurrent_start_stop_outcome_and_restore_preserve_subject_data() {
    let mut f = ConnectedBody::new().await;
    let subject = f.subject("body-new-subject").await;
    let store = open_subjektiv_store(&f.connected.api).unwrap();
    let initial = store.subject_permissions(&subject).unwrap().unwrap();
    assert_eq!(
        initial.execution,
        server_api::authorization::subject_conversation_permissions(TEST_WORKSPACE_ID, &subject)
    );
    assert!(initial.delegation.is_empty());
    assert!(f.read(&subject).await["current"].is_null());

    // Retention inputs are deliberately independent of Worker execution and of
    // Ticket conclusion. Direct Memory setup is not a claimed body mutation.
    let memory = store
        .create_memory(
            &subject,
            crate::subjektiv::MemoryDraft::active(
                memory::extract::CandidateKind::Decision,
                "Keep this decision",
                "Retained Memory body",
                "Survive body stop/restore",
                "retention fixture",
            ),
        )
        .unwrap();
    let tickets = browser_ticket_backend(&f.connected.api).unwrap();
    let ticket = tickets
        .create(NewTicket::new("Body stop does not conclude this case"))
        .unwrap();
    let before_case = tickets.show(ticket.id.clone().into()).unwrap();
    let conversations = format!("/api/w/{TEST_WORKSPACE_ID}/chat/conversations");
    let conversation = f
        .ok(
            "POST",
            &conversations,
            json!({"operation_id":"body-conversation","subject_id":subject}),
        )
        .await;
    let messages = format!(
        "{}/{}/messages",
        conversations,
        conversation["id"].as_str().unwrap()
    );
    let saved = f
        .ok(
            "POST",
            &messages,
            json!({"operation_id":"body-before-start","text":"BEFORE_BODY_START_RETAINED"}),
        )
        .await;
    assert_ne!(saved["delivery"]["state"], "accepted");

    let start = absent_start("body-concurrent-same");
    let path = format!("{}/start", body_path(&subject));
    let (left, right) = tokio::join!(
        f.request("POST", &path, start.clone()),
        f.request("POST", &path, start.clone())
    );
    for (status, result) in [&left, &right] {
        assert_eq!(*status, StatusCode::OK, "exact concurrent retry: {result}");
        assert!(
            matches!(result["disposition"].as_str(), Some("accepted" | "unknown")),
            "{result}"
        );
    }
    // An in-flight response is not assumed to settle the other call. Reconcile
    // with the same request identity, never with a second creation request.
    let replay = f.mutate(&subject, "/start", start.clone()).await;
    assert_eq!(replay["disposition"], "accepted", "{replay}");
    let running = f.read(&subject).await;
    assert_eq!(running["current"]["runtime_id"], CONNECTED_RUNTIME);
    assert_eq!(running["observation"]["kind"], "available");
    assert_eq!(
        running["observation"]["worker"]["profile"],
        server_api::authorization::SUBJECT_CONVERSATION_PROFILE
    );
    assert_eq!(f.worker_count(), 1);
    let current = running["current"].clone();
    let worker = current["worker_id"].as_str().unwrap().to_owned();
    let source = RuntimeWorkerRef::new(CONNECTED_RUNTIME, &worker);
    let singleton = f
        .connected
        .api
        .store
        .require_current_worker_singleton_owner(TEST_WORKSPACE_ID, &source)
        .unwrap()
        .expect("normal body singleton owner");
    assert_eq!(
        serde_json::to_value(subject_body::reference(&singleton)).unwrap(),
        current
    );
    let live_reuse = f
        .mutate(&subject, "/start", intent("body-live-reuse", &running))
        .await;
    assert_eq!(live_reuse["disposition"], "accepted", "{live_reuse}");
    assert_eq!(f.worker_count(), 1);

    let mut events = f.connected.runtime.subscribe_worker_observation().unwrap();
    let delivered = f
        .ok(
            "POST",
            &messages,
            json!({"operation_id":"body-live-message","text":"BODY_CANONICAL_SESSION_MARKER"}),
        )
        .await;
    assert_eq!(delivered["delivery"]["state"], "accepted");
    let input = tokio::time::timeout(DEADLINE, f.inputs.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(input.contains("BODY_CANONICAL_SESSION_MARKER"), "{input}");
    body_idle(&mut events, &worker).await;
    let root = f.connected.runtime_root.join("workers").join(&worker);
    let history = session_store::read_retained_session_snapshot(
        &root,
        &format!("worker-runtime-{worker}"),
        2 * 1024 * 1024,
    )
    .unwrap();
    assert!(
        serde_json::to_string(&history.snapshot)
            .unwrap()
            .contains("BODY_CANONICAL_SESSION_MARKER")
    );
    let attribution = store
        .subject_session_attributions(&subject)
        .unwrap()
        .into_iter()
        .find(|entry| entry.worker_id == worker)
        .expect("signed startup registered real Session");
    assert_eq!(attribution.session_id, history.identity.session_id);
    let host =
        subjektiv::journal::JournalHostContext::validated_session(&store, attribution.clone())
            .unwrap();
    let capture =
        subjektiv_journal::capture_input(&f.connected.api, &attribution, 0, 0, 128 * 1024).unwrap();
    let ranges = subjektiv::journal::recording::capture_ranges(&capture).unwrap();
    assert!(!ranges.is_empty(), "admit actual canonical Session bytes");
    let journal = store
        .accept_journal_sources(&host, "Retained body Journal", &ranges)
        .unwrap();
    let reader =
        subjektiv::journal::JournalHostContext::validated_reader(&store, &subject).unwrap();
    let before_journal =
        serde_json::to_value(store.read_journal(&reader, &journal).unwrap()).unwrap();
    let before_sources = serde_json::to_value(
        store
            .list_journal_sources(
                &reader,
                &journal,
                None,
                subjektiv::journal::MAX_JOURNAL_PAGE,
            )
            .unwrap(),
    )
    .unwrap();
    assert!(
        before_sources["next"].is_null(),
        "the small retention fixture must compare its complete Journal source ledger: {before_sources}"
    );
    assert_eq!(
        before_sources["items"].as_array().unwrap().len(),
        ranges.len(),
        "every admitted canonical source must be included in the retention comparison"
    );
    let before_messages = f.ok("GET", &messages, Value::Null).await;

    let observed = f.read(&subject).await;
    let stop = intent("body-stop-lost-ack", &observed);
    f.lose_stop_ack.store(true, Ordering::SeqCst);
    let (status, lost) = f
        .request(
            "POST",
            &format!("{}/stop", body_path(&subject)),
            stop.clone(),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{lost}");
    // The caller has an unknown result. A GET is only current observation: it
    // cannot prove which request stopped it, nor manufacture a receipt.
    let stopped = f.read(&subject).await;
    assert_eq!(stopped["current"], current);
    assert_eq!(stopped["observation"]["worker"]["state"], "stopped");
    let mut unrelated = stop.clone();
    unrelated["request_id"] = json!("body-stop-never-sent");
    let missing = f.mutate(&subject, "/stop/outcome", unrelated).await;
    assert_eq!(
        missing["outcome"], "not_found",
        "GET must not invent stop acceptance: {missing}"
    );
    let outcome = f.mutate(&subject, "/stop/outcome", stop.clone()).await;
    assert_eq!(
        outcome["outcome"], "stopped",
        "only exact outcome resolves the lost ACK: {outcome}"
    );
    let restore = intent("body-restore-same-generation", &stopped);
    let restored = f.mutate(&subject, "/start", restore.clone()).await;
    assert_eq!(
        restored["disposition"],
        "accepted",
        "{restored}; Runtime errors: {:?}",
        f.connected.runtime_errors.lock().unwrap()
    );
    assert_eq!(restored["body"]["current"], current);
    assert_ne!(
        restored["body"]["observation"]["worker"]["state"],
        "stopped"
    );
    assert_eq!(
        f.worker_count(),
        1,
        "restore must not create a replacement Worker/generation"
    );
    let replay_restore = f.mutate(&subject, "/start", restore).await;
    assert_eq!(
        replay_restore["disposition"], "accepted",
        "{replay_restore}"
    );
    let historical = f.mutate(&subject, "/stop/outcome", stop.clone()).await;
    assert_eq!(historical["outcome"], "stopped");
    assert_ne!(
        historical["body"]["observation"]["worker"]["state"], "stopped",
        "historical stop receipt is not the restored body's current state"
    );
    let stop_replay = f.mutate(&subject, "/stop", stop).await;
    assert_eq!(stop_replay["disposition"], "accepted");
    assert_ne!(
        f.read(&subject).await["observation"]["worker"]["state"],
        "stopped",
        "exact historical stop replay must not stop the restored lifetime"
    );
    for suffix in ["/start", "/stop"] {
        let stale = intent("body-old-observation", &observed);
        let (status, result) = f
            .request("POST", &format!("{}{suffix}", body_path(&subject)), stale)
            .await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "old Runtime observation: {result}"
        );
        assert_failed_request(&result, &subject, "body-old-observation", "not_accepted");
    }

    // Reopen authority rather than treating an earlier model callback as durable evidence.
    let reopened = open_subjektiv_store(&f.connected.api).unwrap();
    assert_eq!(
        reopened.subject_permissions(&subject).unwrap(),
        Some(initial)
    );
    assert_eq!(
        serde_json::to_value(reopened.memory(&subject, &memory.id).unwrap().unwrap()).unwrap(),
        serde_json::to_value(&memory).unwrap()
    );
    assert_eq!(
        serde_json::to_value(reopened.read_journal(&reader, &journal).unwrap()).unwrap(),
        before_journal
    );
    assert_eq!(
        serde_json::to_value(
            reopened
                .list_journal_sources(
                    &reader,
                    &journal,
                    None,
                    subjektiv::journal::MAX_JOURNAL_PAGE
                )
                .unwrap()
        )
        .unwrap(),
        before_sources
    );
    assert_eq!(f.ok("GET", &messages, Value::Null).await, before_messages);
    let after_case = tickets.show(ticket.id.into()).unwrap();
    assert_eq!(
        after_case, before_case,
        "body lifecycle must not conclude or edit the case"
    );
    assert_eq!(
        reopened.subject(&subject).unwrap().unwrap().state,
        crate::subjektiv::SubjectState::Active
    );
    let after_history = session_store::read_retained_session_snapshot(
        &root,
        &format!("worker-runtime-{worker}"),
        2 * 1024 * 1024,
    )
    .unwrap();
    assert_eq!(
        after_history.identity.session_id,
        history.identity.session_id
    );
    assert!(
        serde_json::to_string(&after_history.snapshot)
            .unwrap()
            .contains("BODY_CANONICAL_SESSION_MARKER")
    );
    f.stop(&subject, "body-final-stop").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn body_distinct_concurrent_starts_keep_one_owner_and_reject_foreign_targets() {
    let f = ConnectedBody::new().await;
    let subject = f.subject("body-distinct-subject").await;
    let other = f.subject("body-foreign-subject").await;
    let path = format!("{}/start", body_path(&subject));
    let (a, b) = tokio::join!(
        f.request("POST", &path, absent_start("body-click-a")),
        f.request("POST", &path, absent_start("body-click-b"))
    );
    assert!(
        [&a, &b]
            .iter()
            .any(|(status, body)| *status == StatusCode::OK && body["disposition"] == "accepted"),
        "one request must start the body: {a:?}, {b:?}"
    );
    assert_eq!(
        [&a, &b]
            .iter()
            .filter(|(status, _)| *status == StatusCode::CONFLICT)
            .count(),
        1,
        "a distinct absent-start loser is terminal, not an unknown accepted operation: {a:?}, {b:?}"
    );
    for (request_id, (status, body)) in [("body-click-a", &a), ("body-click-b", &b)] {
        if *status == StatusCode::CONFLICT {
            assert_failed_request(body, &subject, request_id, "superseded");
            let (retry_status, retry) = f.request("POST", &path, absent_start(request_id)).await;
            assert_eq!(retry_status, StatusCode::CONFLICT, "{retry}");
            assert_failed_request(&retry, &subject, request_id, "superseded");
        } else {
            assert_eq!(*status, StatusCode::OK, "{body}");
            assert_eq!(body["disposition"], "accepted", "{body}");
        }
    }
    assert_eq!(f.worker_count(), 1);
    let current = f.read(&subject).await;
    let foreign = intent("body-foreign-intent", &current);
    for suffix in ["/start", "/stop", "/stop/outcome"] {
        let (status, result) = f
            .request(
                "POST",
                &format!("{}{suffix}", body_path(&other)),
                foreign.clone(),
            )
            .await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "cross-Subject {suffix}: {result}"
        );
        assert_failed_request(&result, &other, "body-foreign-intent", "superseded");
    }
    let mut old_generation = foreign.clone();
    old_generation["expected_current"]["generation"] = json!(
        (current["current"]["generation"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            + 1)
        .to_string()
    );
    for suffix in ["/start", "/stop", "/stop/outcome"] {
        let (status, result) = f
            .request(
                "POST",
                &format!("{}{suffix}", body_path(&subject)),
                old_generation.clone(),
            )
            .await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "stale generation {suffix}: {result}"
        );
        assert_failed_request(&result, &subject, "body-foreign-intent", "superseded");
    }
    let reader = seed_test_api_token(f.connected.api.store.as_ref(), "connected-body-reader");
    for (method, suffix, body) in [
        ("GET", "", None),
        ("POST", "/start", Some(foreign.clone())),
        ("POST", "/stop", Some(foreign.clone())),
        ("POST", "/stop/outcome", Some(foreign.clone())),
    ] {
        let (status, result) = response(
            &f.connected.api,
            human_request(
                &reader,
                method,
                &format!("{}{suffix}", body_path(&subject)),
                body,
            ),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "non-owner {suffix}: {result}"
        );
        assert!(
            result["request"].is_null(),
            "authorization before action admission supplies no request resolution: {result}"
        );
    }
    let wrong_workspace = body_path(&subject).replace(TEST_WORKSPACE_ID, "not-this-workspace");
    let (status, result) = f
        .request("POST", &format!("{wrong_workspace}/stop"), foreign)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{result}");
    assert_eq!(f.read(&subject).await["current"], current["current"]);
    assert!(f.read(&other).await["current"].is_null());
    assert_eq!(f.worker_count(), 1);
    f.stop(&subject, "body-distinct-cleanup").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn body_start_and_restore_do_not_repair_missing_session_record_permission() {
    let f = ConnectedBody::new().await;
    let subject = f.subject("body-revoked-subject").await;
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{subject}/permissions");
    let initial = open_subjektiv_store(&f.connected.api)
        .unwrap()
        .subject_permissions(&subject)
        .unwrap()
        .unwrap();
    let execution = initial
        .execution
        .iter()
        .filter(|permission| permission.operation != "session.record")
        .cloned()
        .collect::<Vec<_>>();
    let revoked = f.ok("POST", &path, json!({"operation_id":"body-remove-session-record",
        "expected_change_id":initial.change_id,"execution":execution,"delegation":initial.delegation})).await;
    let (status, denied) = f
        .request(
            "POST",
            &format!("{}/start", body_path(&subject)),
            absent_start("body-denied-start"),
        )
        .await;
    assert!(
        status == StatusCode::FORBIDDEN
            || (status == StatusCode::OK && denied["disposition"] != "accepted"),
        "required startup authorization must refuse: {status}: {denied}"
    );
    assert!(
        denied.to_string().contains("session.record"),
        "actionable missing-operation diagnostic: {denied}"
    );
    assert_eq!(
        f.ok("GET", &path, Value::Null).await["permissions"],
        revoked["permissions"]
    );
    assert!(
        open_subjektiv_store(&f.connected.api)
            .unwrap()
            .subject_session_attributions(&subject)
            .unwrap()
            .is_empty(),
        "refused startup must not register a protected Session"
    );

    // A separate Subject proves the same denial at restore, with normal defaults
    // at creation and explicit owner revocation only after a successful start.
    let subject = f.subject("body-restore-revoked-subject").await;
    let started = f
        .mutate(&subject, "/start", absent_start("body-authorized-start"))
        .await;
    assert_eq!(started["disposition"], "accepted", "{started}");
    f.stop(&subject, "body-before-revoked-restore").await;
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{subject}/permissions");
    let policy = open_subjektiv_store(&f.connected.api)
        .unwrap()
        .subject_permissions(&subject)
        .unwrap()
        .unwrap();
    let execution = policy
        .execution
        .iter()
        .filter(|permission| permission.operation != "session.record")
        .cloned()
        .collect::<Vec<_>>();
    let revoked = f.ok("POST", &path, json!({"operation_id":"body-revoke-before-restore",
        "expected_change_id":policy.change_id,"execution":execution,"delegation":policy.delegation})).await;
    let observed = f.read(&subject).await;
    let request = intent("body-denied-restore", &observed);
    let denied = f.mutate(&subject, "/start", request.clone()).await;
    assert_ne!(denied["disposition"], "accepted", "{denied}");
    let replay = f.mutate(&subject, "/start", request).await;
    assert_ne!(replay["disposition"], "accepted", "{replay}");
    assert_eq!(
        f.ok("GET", &path, Value::Null).await["permissions"],
        revoked["permissions"]
    );
    assert_eq!(
        f.read(&subject).await["current"],
        observed["current"],
        "denial must not replace singleton generation"
    );
    assert_eq!(
        f.read(&subject).await["observation"]["worker"]["state"],
        "stopped"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn body_retained_stop_and_restore_input_mismatches_remain_unresolved() {
    let f = ConnectedBody::new().await;
    let subject = f.subject("body-input-mismatch-subject").await;
    f.mutate(
        &subject,
        "/start",
        absent_start("body-input-mismatch-start"),
    )
    .await;
    let original_stop = f.stop(&subject, "body-owned-stop").await;
    let mut changed_stop = original_stop.clone();
    changed_stop["expected_observation_token"] = json!("different-input-same-stop-id");
    for suffix in ["/stop", "/stop/outcome"] {
        let (status, error) = f
            .request(
                "POST",
                &format!("{}{suffix}", body_path(&subject)),
                changed_stop.clone(),
            )
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "{error}");
        assert_failed_request(&error, &subject, "body-owned-stop", "unresolved");
    }
    let stopped = f.read(&subject).await;
    let original_restore = intent("body-owned-restore", &stopped);
    let restored = f.mutate(&subject, "/start", original_restore.clone()).await;
    assert_eq!(restored["disposition"], "accepted", "{restored}");
    let mut changed_restore = original_restore.clone();
    changed_restore["expected_observation_token"] = json!("different-input-same-restore-id");
    let (status, error) = f
        .request(
            "POST",
            &format!("{}/start", body_path(&subject)),
            changed_restore,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    assert_failed_request(&error, &subject, "body-owned-restore", "unresolved");
    let replay = f.mutate(&subject, "/start", original_restore).await;
    assert_eq!(replay["request_id"], "body-owned-restore");
    assert_eq!(replay["disposition"], "accepted", "{replay}");
    assert_eq!(f.read(&subject).await["current"], stopped["current"]);
    assert_eq!(f.worker_count(), 1);
    f.stop(&subject, "body-input-mismatch-cleanup").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn body_pending_lifecycle_conflicts_do_not_claim_stale_request_nonacceptance() {
    let f = ConnectedBody::new().await;
    let subject = f.subject("body-busy-subject").await;
    f.mutate(&subject, "/start", absent_start("body-busy-start"))
        .await;
    let running = f.read(&subject).await;
    let pending = intent("body-pending-stop", &running);
    let source = RuntimeWorkerRef::new(
        CONNECTED_RUNTIME,
        running["current"]["worker_id"].as_str().unwrap(),
    );
    let api = f.connected.api.clone();
    let token = pending["expected_observation_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let runtime_request = WorkerLifecycleRequest {
        reason: Some("Stopped from the Subject screen; Subject data is retained".into()),
        ticket_assignment: None,
        stop_intent: Some(runtime_api::WorkerStopIntent {
            expected_observation_token: token,
            request_id: "body-pending-stop".into(),
        }),
    };
    let admission_source = source.clone();
    let admission_request = runtime_request.clone();
    // Use the production signed Runtime admission, without executing cleanup yet.
    // The raw Runtime intent is not in the browser caller's hashed namespace.
    let admission = tokio::task::spawn_blocking(move || {
        api.runtime
            .coordinate_worker_stop(&admission_source, admission_request)
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(admission, runtime_api::WorkerStopOutcome::Pending);
    let occupied = f.read(&subject).await;
    let request = intent("body-busy-restore", &occupied);
    for _ in 0..2 {
        let (status, error) = f
            .request(
                "POST",
                &format!("{}/start", body_path(&subject)),
                request.clone(),
            )
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "{error}");
        assert_failed_request(&error, &subject, "body-busy-restore", "unresolved");
    }
    assert_eq!(f.read(&subject).await["current"], running["current"]);
    assert_eq!(f.worker_count(), 1);
    let api = f.connected.api.clone();
    tokio::task::spawn_blocking(move || {
        assert_eq!(
            api.runtime
                .worker_stop_outcome(&source, runtime_request.clone())
                .unwrap(),
            runtime_api::WorkerStopOutcome::Pending
        );
        let completed = api
            .runtime
            .stop_worker(&source, runtime_request.clone())
            .unwrap();
        assert_eq!(completed.state, InternalWorkerOperationState::Accepted);
        assert!(matches!(
            api.runtime
                .worker_stop_outcome(&source, runtime_request)
                .unwrap(),
            runtime_api::WorkerStopOutcome::Stopped { .. }
        ));
    })
    .await
    .unwrap();
}
