// Included as a child module of server::tests. These tests exercise committed
// Assign + HTTP + embedded Runtime boundaries, not the old Queue predicate.
use super::*;
use session_store::{LogEntry, Store, WorkerSessionStore};
use worker_runtime::execution as delivery_execution;

fn delivery_assignment_path(ticket: &str) -> String {
    format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator")
}

async fn delivery_http(
    api: &WorkspaceApi,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: StatusCode,
) -> Value {
    let token = format!("api-token-{TEST_WORKSPACE_ID}");
    let response = build_router(api.clone())
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(body.map(|v| v.to_string()).unwrap_or_default()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        status,
        expected,
        "{method} {path}: {}",
        String::from_utf8_lossy(&bytes)
    );
    if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    }
}

async fn delivery_assign(api: &WorkspaceApi, ticket: &str, key: &str) -> Value {
    delivery_http(
        api,
        "PUT",
        &delivery_assignment_path(ticket),
        Some(json!({
            "operation_id": key,
            "principal": {"kind":"workspace_agent", "agent_key":"workspace-orchestrator"}
        })),
        StatusCode::OK,
    )
    .await
}

fn delivery_ticket(api: &WorkspaceApi, title: &str) -> ticket::TicketRef {
    browser_ticket_backend(api)
        .unwrap()
        .create(ticket::NewTicket::new(title))
        .unwrap()
}

fn delivery_tracking(
    api: &WorkspaceApi,
    ticket: &str,
    assignment: &str,
) -> ticket::TicketAssignmentTrackingProjection {
    api.store
        .get_ticket_assignment_tracking(TEST_WORKSPACE_ID, ticket, assignment)
        .unwrap()
        .unwrap()
}

fn delivery_worker(response: BrowserWorkspaceOrchestratorResponse) -> RuntimeWorkerRef {
    assert!(response.online);
    RuntimeWorkerRef::new(
        EMBEDDED_WORKER_RUNTIME_ID,
        response.worker.unwrap().worker_id,
    )
}

fn delivery_scan(api: &WorkspaceApi, worker: &RuntimeWorkerRef) {
    dispatch_orchestrator_assignment_attention(api, worker);
    use protocol::subscription::SubscriptionWorkerState as S;
    maybe_dispatch_orchestrator_turn_end(api, &worker.worker_id, Some(S::Running), S::Idle);
}

fn delivery_stop(api: &WorkspaceApi, worker: &RuntimeWorkerRef) {
    api.runtime
        .stop_worker(
            worker,
            WorkerLifecycleRequest {
                reason: None,
                ticket_assignment: None,
            },
        )
        .unwrap();
}

async fn delivery_recording_fixture(
    root: &Path,
) -> (
    WorkspaceApi,
    Arc<DeterministicExecutionBackend>,
    RuntimeWorkerRef,
    RuntimeIdentityMaterial,
) {
    let (mut api, execution) = test_api_with_recording_backend(root).await;
    seed_test_api_token(api.store.as_ref(), TEST_WORKSPACE_ID);
    let worker = delivery_worker(start_attention_test_orchestrator(&api).await);
    let identity = RuntimeIdentityMaterial::generate(EMBEDDED_WORKER_RUNTIME_ID).unwrap();
    configure_runtime_request_auth(&mut api, &identity, EMBEDDED_WORKER_RUNTIME_ID);
    seed_worker_source_member(&api, &worker.runtime_id, &worker.worker_id);
    assert!(
        execution.take_inputs().is_empty(),
        "open tickets are not implicit delegations"
    );
    (api, execution, worker, identity)
}

async fn delivery_self_report(
    api: &WorkspaceApi,
    identity: &RuntimeIdentityMaterial,
    worker: &RuntimeWorkerRef,
    current: &ticket::TicketAssignmentTrackingProjection,
    status: &str,
    key: &str,
) -> Value {
    let path = format!(
        "/workspaces/{TEST_WORKSPACE_ID}/tickets/{}/assignment-tracking",
        current.ticket_id
    );
    let body = serde_json::to_vec(&json!({
        "assignment_id":current.assignment_id,
        "expected_last_event_id":current.last_event_id,
        "operation_key":key,
        "tracking": {"status":status, "waiting_reason":"Need the human's choice",
            "progress":"Options investigated", "next_action":"Read the answer then decide",
            "result": if status == "done" { Some("Returned options without closing") } else { None }}
    })).unwrap();
    let response = build_router(api.clone())
        .oneshot(runtime_source_request(
            identity,
            Some(&worker.worker_id),
            "POST",
            &path,
            body,
        ))
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).unwrap()
}

fn delivery_external_comment(api: &WorkspaceApi, ticket: &str, text: &str) {
    browser_ticket_backend(api)
        .unwrap()
        .add_event(
            ticket.to_owned().into(),
            ticket::NewTicketEvent::new(ticket::TicketEventKind::Comment, text),
        )
        .unwrap();
    notify_ticket_recipients(api, TEST_WORKSPACE_ID, ticket, "open", "open", None);
}

#[tokio::test]
async fn assignment_waiting_self_report_does_not_loop_but_external_answer_is_delivered() {
    let dir = tempfile::tempdir().unwrap();
    let (api, execution, worker, identity) = delivery_recording_fixture(dir.path()).await;
    let ticket = delivery_ticket(&api, "Choose an option");
    let assigned = delivery_assign(&api, &ticket.id, "assign-choice").await;
    let assignment = assigned["assignment"]["assignment_id"].as_str().unwrap();
    let initial = delivery_tracking(&api, &ticket.id, assignment);
    assert_eq!(
        initial.tracking.as_ref().unwrap().status,
        ticket::TicketAssignmentTrackingStatus::Waiting
    );
    let first = execution.take_inputs();
    assert_eq!(first.len(), 1, "Assign alone delivers without Queue");
    let first_ids = execution.take_input_request_ids();
    assert!(first_ids[0].is_some());
    assert!(first[0].1.contains(ticket.resource_key.as_deref().unwrap()));
    let reported =
        delivery_self_report(&api, &identity, &worker, &initial, "waiting", "wait-human").await;
    // Even an explicit notification of one's own mutation must not manufacture
    // fresh work, and repeated idle/startup scans must not use waiting as a queue.
    notify_ticket_recipients(
        &api,
        TEST_WORKSPACE_ID,
        &ticket.id,
        "open",
        "open",
        Some(worker.clone()),
    );
    for _ in 0..3 {
        delivery_scan(&api, &worker);
    }
    assert!(
        execution.take_inputs().is_empty(),
        "self progress became an input loop"
    );
    let waiting = delivery_tracking(&api, &ticket.id, assignment);
    assert_eq!(serde_json::to_value(&waiting).unwrap(), reported);
    delivery_external_comment(&api, &ticket.id, "Human answer: choose option B");
    let answer = execution.take_inputs();
    assert_eq!(
        answer.len(),
        1,
        "external answer must wake the existing delegation"
    );
    assert_ne!(execution.take_input_request_ids(), first_ids);
    assert!(
        answer[0]
            .1
            .contains(ticket.resource_key.as_deref().unwrap())
    );
    assert!(
        answer[0].1.contains("Need the human's choice"),
        "waiting context missing: {:?}",
        answer[0].1
    );
    assert!(answer[0].1.contains("Read the answer then decide"));
    for _ in 0..3 {
        delivery_scan(&api, &worker);
    }
    assert!(
        execution.take_inputs().is_empty(),
        "same answer was delivered again"
    );
    assert_eq!(
        delivery_tracking(&api, &ticket.id, assignment),
        waiting,
        "delivery/idle must not infer handling inprogress or done"
    );
    delivery_stop(&api, &worker);
}

#[tokio::test]
async fn assignment_done_historical_inputs_do_not_restart_work_but_reissue_is_new_delegation() {
    let dir = tempfile::tempdir().unwrap();
    let (api, execution, worker, identity) = delivery_recording_fixture(dir.path()).await;
    let ticket = delivery_ticket(&api, "Return options while open");
    let assigned = delivery_assign(&api, &ticket.id, "assign-return").await;
    let assignment = assigned["assignment"]["assignment_id"].as_str().unwrap();
    execution.take_inputs();
    execution.take_input_request_ids();
    let initial = delivery_tracking(&api, &ticket.id, assignment);
    delivery_self_report(&api, &identity, &worker, &initial, "done", "return-options").await;
    let done = delivery_tracking(&api, &ticket.id, assignment);
    assert_eq!(
        api.authority.ticket(&ticket.id).unwrap().state.as_str(),
        "open"
    );
    delivery_external_comment(
        &api,
        &ticket.id,
        "Historical result annotation, not a new request",
    );
    for _ in 0..3 {
        delivery_scan(&api, &worker);
    }
    assert!(
        execution.take_inputs().is_empty(),
        "open + done must not execute history"
    );
    assert_eq!(delivery_tracking(&api, &ticket.id, assignment), done);
    for (key, action) in [
        (
            "close-return",
            json!({"action":"close", "reason":"not_planned"}),
        ),
        ("reopen-return", json!({"action":"reopen"})),
    ] {
        let observed = api.authority.ticket(&ticket.id).unwrap();
        delivery_http(
            &api,
            "POST",
            &format!("/workspaces/{TEST_WORKSPACE_ID}/tickets/{}/case", ticket.id),
            Some(json!({"expected_content_digest":observed.content_digest,
                "expected_state":observed.state,
                "expected_closure_operation_key":observed.closure_operation_key,
                "operation_key":key, "action":action, "explanation":"Human case decision"})),
            StatusCode::OK,
        )
        .await;
        delivery_scan(&api, &worker);
        assert!(
            execution.take_inputs().is_empty(),
            "case history must not revive done delegation"
        );
        assert_eq!(delivery_tracking(&api, &ticket.id, assignment), done);
    }
    let reassigned = delivery_assign(&api, &ticket.id, "reissue-return").await;
    let new_assignment = reassigned["assignment"]["assignment_id"].as_str().unwrap();
    assert_ne!(new_assignment, assignment);
    assert_eq!(
        execution.take_inputs().len(),
        1,
        "explicit reissue must be reconsidered"
    );
    let retained = delivery_tracking(&api, &ticket.id, assignment);
    assert!(!retained.current);
    assert_eq!(
        retained.tracking, done.tracking,
        "new request overwrote the old result"
    );
    assert_eq!(
        delivery_tracking(&api, &ticket.id, new_assignment)
            .tracking
            .unwrap()
            .status,
        ticket::TicketAssignmentTrackingStatus::Waiting
    );
    // An exact old Assign retry returns its receipt, never revives old work.
    assert_eq!(
        delivery_assign(&api, &ticket.id, "assign-return").await,
        assigned
    );
    delivery_scan(&api, &worker);
    assert!(execution.take_inputs().is_empty());
    delivery_stop(&api, &worker);
}

#[tokio::test]
async fn assignment_clear_and_reassign_discard_old_unknown_delivery_without_stopping_worker() {
    for clear_first in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let (api, execution, worker, _) = delivery_recording_fixture(dir.path()).await;
        let ticket = delivery_ticket(&api, "Fence old delivery");
        execution.reject_inputs("unknown acceptance response");
        let assigned = delivery_assign(&api, &ticket.id, "assign-old").await;
        let assignment = assigned["assignment"]["assignment_id"].as_str().unwrap();
        let old_attempts = execution.take_inputs();
        let old_ids = execution.take_input_request_ids();
        assert!(!old_attempts.is_empty());
        assert!(old_ids.iter().all(|id| id == &old_ids[0]));
        if clear_first {
            delivery_http(
                &api,
                "DELETE",
                &format!(
                    "{}?assignment_id={assignment}&operation_id=clear-old",
                    delivery_assignment_path(&ticket.id)
                ),
                None,
                StatusCode::OK,
            )
            .await;
        }
        *execution.input_failure.lock().unwrap() = None;
        // No extra scan, old-operation retry, or synthetic turn end between
        // clearing/reissuing the failed input and this committed new Assign.
        let reassigned = delivery_assign(&api, &ticket.id, "assign-new").await;
        let new_assignment = reassigned["assignment"]["assignment_id"].as_str().unwrap();
        assert_ne!(new_assignment, assignment);
        let inputs = execution.take_inputs();
        assert_eq!(inputs.len(), 1, "new Assign must not need another scan");
        assert!(inputs[0].1.contains(new_assignment));
        let ids = execution.take_input_request_ids();
        assert_eq!(ids.len(), 1);
        assert_ne!(ids[0], old_ids[0], "new assignment reused stale input identity");
        assert_eq!(
            delivery_assign(&api, &ticket.id, "assign-old").await,
            assigned
        );
        assert!(execution.take_inputs().is_empty(), "old retry revived work");
        let old = delivery_tracking(&api, &ticket.id, assignment);
        assert!(!old.current);
        assert_eq!(
            old.tracking.unwrap().status,
            ticket::TicketAssignmentTrackingStatus::Waiting,
            "clear/reissue must not rewrite the old status to done"
        );
        assert_eq!(
            find_workspace_orchestrator(&api).unwrap().worker,
            worker,
            "clear/reassign are not Worker stop or replacement"
        );
        delivery_stop(&api, &worker);
    }
}

#[tokio::test]
async fn assignment_or_answer_committed_during_empty_scan_is_delivered_without_another_trigger() {
    for answer in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let (api, execution, worker, identity) = delivery_recording_fixture(dir.path()).await;
        let ticket = delivery_ticket(&api, "Input racing an empty scan");
        if answer {
            let assigned = delivery_assign(&api, &ticket.id, "initial-assign").await;
            let current = delivery_tracking(
                &api,
                &ticket.id,
                assigned["assignment"]["assignment_id"].as_str().unwrap(),
            );
            delivery_self_report(&api, &identity, &worker, &current, "waiting", "wait-answer").await;
            execution.take_inputs();
            execution.take_input_request_ids();
        }
        assert!(attention_pending(&api).is_empty());
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let caller_api = api.clone();
            let caller_worker = worker.clone();
            let caller = scope.spawn(move || {
                let mut checks = 0;
                with_orchestrator_attention_reservation(
                    &caller_api,
                    &caller_worker,
                    true,
                    |state, pending| {
                        check_orchestrator_assignment_attention(
                            &caller_api, &caller_worker, state, pending,
                        );
                        if checks == 0 {
                            // The production scan has read an empty snapshot but
                            // still owns its reservation. Race a real HTTP commit.
                            entered_tx.send(()).unwrap();
                            release_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
                        }
                        checks += 1;
                    },
                );
                assert_eq!(checks, 2, "committed input must defer a scan");
            });
            entered_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            if answer {
                futures::executor::block_on(delivery_http(
                    &api,
                    "POST",
                    &format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{}/thread-events", ticket.id),
                    Some(serde_json::to_value(NewTicketEvent::new(
                        TicketEventKind::Comment, "Human answer: go ahead",
                    )).unwrap()),
                    StatusCode::NO_CONTENT,
                ));
            } else {
                futures::executor::block_on(delivery_assign(&api, &ticket.id, "racing-assign"));
            }
            // No manual dispatch, Restore, or synthetic Running -> Idle.
            assert!(execution.take_inputs().is_empty());
            release_tx.send(()).unwrap();
            caller.join().unwrap();
        });
        let inputs = execution.take_inputs();
        assert_eq!(inputs.len(), 1, "idle Orchestrator was stranded");
        assert!(inputs[0].1.contains(ticket.resource_key.as_deref().unwrap()));
        if answer {
            assert!(inputs[0].1.contains("Need the human's choice"));
        }
        assert_eq!(execution.take_input_request_ids().len(), 1);
        assert!(attention_pending(&api).is_empty());
        delivery_stop(&api, &worker);
    }
}

#[tokio::test]
async fn assignment_failed_delivery_and_its_reentrant_idle_do_not_schedule_retry_loop() {
    let dir = tempfile::tempdir().unwrap();
    let (api, execution, worker, _) = delivery_recording_fixture(dir.path()).await;
    let ticket = delivery_ticket(&api, "No failed-delivery busy loop");
    execution.reject_inputs("Runtime currently cannot accept notifications");
    let callback_api = api.clone();
    execution.before_input_returns(move |worker| {
        use protocol::subscription::SubscriptionWorkerState as S;
        maybe_dispatch_orchestrator_turn_end(
            &callback_api, &worker.worker_id.to_string(), Some(S::Running), S::Idle,
        );
    });
    delivery_assign(&api, &ticket.id, "assign-failed").await;
    assert_eq!(execution.take_inputs().len(), 2, "idle must not add retries beyond the bounded send");
    let ids = execution.take_input_request_ids();
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], ids[1]);
    assert_eq!(attention_pending(&api).len(), 1, "failure must retain durable input");
    delivery_stop(&api, &worker);
}

// The model boundary is scripted, but Controller, notification acceptance,
// canonical session commits, Runtime management and HTTP routing are real.
#[derive(Clone, Default)]
struct DeliveryScriptedClient {
    requests: Arc<Mutex<Vec<agen::llm_client::Request>>>,
}

#[async_trait::async_trait]
impl agen::llm_client::LlmClient for DeliveryScriptedClient {
    fn clone_boxed(&self) -> Box<dyn agen::llm_client::LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        request: agen::llm_client::Request,
    ) -> std::result::Result<agen::llm_client::client::ResponseStream, agen::llm_client::ClientError>
    {
        self.requests.lock().unwrap().push(request);
        use agen::llm_client::event::Event as LlmEvent;
        Ok(Box::pin(futures::stream::iter(
            [
                LlmEvent::text_block_start(0),
                LlmEvent::text_delta(
                    0,
                    "Read the delegation; returning without starting a Coder.",
                ),
                LlmEvent::text_block_stop(0, None),
            ]
            .into_iter()
            .map(Ok),
        )))
    }
}

struct DeliveryWorkerFactory {
    root: PathBuf,
    client: DeliveryScriptedClient,
}

impl DeliveryWorkerFactory {
    async fn controller(
        &self,
        worker_id: String,
        restore: bool,
    ) -> std::result::Result<worker_runtime::worker_backend::RuntimeWorkerController, String> {
        let name = format!("delivery-{worker_id}");
        let root = self.root.join("workers").join(&worker_id);
        let store = session_store::CombinedStore::new(
            WorkerSessionStore::new(root.join("session")).map_err(|e| e.to_string())?,
            session_store::WorkerAggregateStore::new(&root, &name).map_err(|e| e.to_string())?,
        );
        let manifest = manifest::WorkerManifest::from_toml(&format!(
            r#"
            [worker]
            name = "{name}"
            pwd = "./"
            [model]
            scheme = "anthropic"
            model_id = "scripted-delivery"
            auth = {{ kind = "none" }}
            [engine]
            max_tokens = 64
            [scope]
            allow = []
        "#
        ))
        .map_err(|e| e.to_string())?;
        let context = worker::WorkerWorkspaceContext::local_filesystem(None);
        let workspace_client = context.client_handle();
        let bootstrap = worker::WorkerBootstrap::new(
            manifest,
            store,
            worker::PromptCatalogSource::builtins_only(),
            context,
            worker::WorkerFilesystemAuthority::None,
            worker::WorkerBootstrapLayout::RuntimeManagedRun {
                run_dir: root.join(uuid::Uuid::now_v7().to_string()),
                bash_output_dir: root.join("bash-output"),
            },
            worker::WorkerControllerTransport::InProcess,
        )
        .with_model_client(self.client.clone());
        let prepared = if restore {
            bootstrap.prepare_restored(&name).await
        } else {
            bootstrap.prepare().await
        }
        .map_err(|e| e.to_string())?;
        let started = prepared.start().await.map_err(|e| e.to_string())?;
        Ok(worker_runtime::worker_backend::RuntimeWorkerController {
            handle: started.handle,
            shutdown: Arc::new(tokio::sync::Mutex::new(Some(started.shutdown))),
            controller_task: started.controller_task,
            workspace_client,
        })
    }
}

#[async_trait::async_trait]
impl worker_runtime::worker_backend::RuntimeWorkerFactory for DeliveryWorkerFactory {
    async fn spawn_controller(
        &self,
        request: delivery_execution::WorkerExecutionSpawnRequest,
    ) -> std::result::Result<worker_runtime::worker_backend::RuntimeWorkerController, String> {
        self.controller(request.worker_ref.worker_id.to_string(), false)
            .await
    }
    async fn restore_controller(
        &self,
        request: delivery_execution::WorkerExecutionRestoreRequest,
    ) -> std::result::Result<worker_runtime::worker_backend::RuntimeWorkerController, String> {
        self.controller(request.worker_ref.worker_id.to_string(), true)
            .await
    }
}

// Lose the response only AFTER the real Worker accepts the exact input. Unlike
// a pre-send rejection, this exercises durable Worker receipt deduplication.
struct DeliveryResponseLossBackend {
    inner: Arc<dyn delivery_execution::WorkerExecutionBackend>,
    lose_responses: std::sync::atomic::AtomicBool,
    attempts: Mutex<Vec<(String, String)>>,
}

impl delivery_execution::WorkerExecutionBackend for DeliveryResponseLossBackend {
    fn backend_id(&self) -> &str {
        self.inner.backend_id()
    }
    fn spawn_worker(
        &self,
        r: delivery_execution::WorkerExecutionSpawnRequest,
    ) -> delivery_execution::WorkerExecutionSpawnResult {
        self.inner.spawn_worker(r)
    }
    fn preflight_restore(
        &self,
        r: &delivery_execution::WorkerExecutionRestoreRequest,
    ) -> std::result::Result<(), delivery_execution::WorkerExecutionResult> {
        self.inner.preflight_restore(r)
    }
    fn restore_worker(
        &self,
        r: delivery_execution::WorkerExecutionRestoreRequest,
    ) -> delivery_execution::WorkerExecutionSpawnResult {
        self.inner.restore_worker(r)
    }
    fn reconcile_restore(
        &self,
        r: delivery_execution::WorkerExecutionRestoreRequest,
    ) -> delivery_execution::WorkerExecutionSpawnResult {
        self.inner.reconcile_restore(r)
    }
    fn activate_restored_worker(
        &self,
        id: delivery_execution::WorkerLifecycleOperationId,
        worker: &worker_runtime::identity::WorkerRef,
    ) -> std::result::Result<(), String> {
        self.inner.activate_restored_worker(id, worker)
    }
    fn dispatch_input(
        &self,
        worker: &worker_runtime::identity::WorkerRef,
        input: worker_runtime::interaction::WorkerInput,
    ) -> delivery_execution::WorkerExecutionResult {
        self.attempts.lock().unwrap().push((
            input.submission_request_id.clone().unwrap(),
            input.content.clone(),
        ));
        let result = self.inner.dispatch_input(worker, input);
        if result.is_accepted()
            && self
                .lose_responses
                .load(std::sync::atomic::Ordering::SeqCst)
        {
            delivery_execution::WorkerExecutionResult::errored(
                delivery_execution::WorkerExecutionOperation::Input,
                "accepted response lost",
            )
        } else {
            result
        }
    }
    fn dispatch_method(
        &self,
        worker: &worker_runtime::identity::WorkerRef,
        method: protocol::Method,
    ) -> delivery_execution::WorkerExecutionResult {
        self.inner.dispatch_method(worker, method)
    }
    fn stop_worker_operation(
        &self,
        r: delivery_execution::WorkerExecutionStopRequest,
    ) -> delivery_execution::WorkerExecutionResult {
        self.inner.stop_worker_operation(r)
    }
    fn worker_snapshot(
        &self,
        worker: &worker_runtime::identity::WorkerRef,
    ) -> Option<protocol::Event> {
        self.inner.worker_snapshot(worker)
    }
}

async fn delivery_real_fixture(
    root: &Path,
    lose_responses: bool,
) -> (
    WorkspaceApi,
    Arc<DeliveryResponseLossBackend>,
    DeliveryScriptedClient,
) {
    let config = test_server_config(root);
    let store = Arc::new(SqliteWorkspaceStore::open_for_test(&config.database_path).unwrap());
    seed_test_registered_workspace(store.as_ref(), &config)
        .await
        .unwrap();
    let client = DeliveryScriptedClient::default();
    let inner = Arc::new(
        worker_runtime::worker_backend::WorkerRuntimeExecutionBackend::new(DeliveryWorkerFactory {
            root: config.embedded_runtime_store_root.clone(),
            client: client.clone(),
        })
        .unwrap(),
    );
    let execution = Arc::new(DeliveryResponseLossBackend {
        inner,
        lose_responses: std::sync::atomic::AtomicBool::new(lose_responses),
        attempts: Mutex::new(Vec::new()),
    });
    let mut api = WorkspaceApi::new_with_execution_backend(config, store, execution.clone())
        .await
        .unwrap();
    // The injectable constructor does not install production Runtime trust.
    // Delivery resolves the registered singleton plus its active binding.
    let identity = RuntimeIdentityMaterial::generate(EMBEDDED_WORKER_RUNTIME_ID).unwrap();
    configure_runtime_request_auth(&mut api, &identity, EMBEDDED_WORKER_RUNTIME_ID);
    seed_test_api_token(api.store.as_ref(), TEST_WORKSPACE_ID);
    (api, execution, client)
}

fn delivery_session_entries(api: &WorkspaceApi, worker: &RuntimeWorkerRef) -> Vec<LogEntry> {
    let session = WorkerSessionStore::open_read_only(
        api.config
            .embedded_runtime_store_root
            .join("workers")
            .join(&worker.worker_id)
            .join("session"),
    )
    .unwrap();
    let id = session.session_id().unwrap().unwrap();
    session
        .list_segments(id)
        .unwrap()
        .into_iter()
        .flat_map(|segment| session.read_all_read_only(id, segment).unwrap())
        .collect()
}

fn delivery_receipts(api: &WorkspaceApi, worker: &RuntimeWorkerRef) -> Vec<Value> {
    delivery_session_entries(api, worker)
        .iter()
        .rev()
        .find_map(|entry| match entry {
            LogEntry::Extension {
                domain, payload, ..
            } if domain == "worker.pending_activations.v1" => {
                Some(payload["notification_receipts"].as_array().unwrap().clone())
            }
            LogEntry::AnnotatedSystemItem { extensions, .. } => extensions
                .iter()
                .find(|e| e.domain == "worker.pending_activations.v1")
                .map(|e| {
                    e.payload["notification_receipts"]
                        .as_array()
                        .unwrap()
                        .clone()
                }),
            _ => None,
        })
        .unwrap_or_default()
}

fn delivery_commits(api: &WorkspaceApi, worker: &RuntimeWorkerRef) -> Vec<String> {
    delivery_session_entries(api, worker)
        .into_iter()
        .filter_map(|entry| match entry {
            LogEntry::AnnotatedSystemItem { entry, .. } => match entry.item {
                session_store::SystemItem::Notification { message, .. } => Some(message),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

async fn delivery_wait_idle(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    expected_commits: usize,
) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let commits = delivery_commits(api, worker).len();
            assert!(
                commits <= expected_commits,
                "duplicate canonical delivery: {:?}",
                delivery_commits(api, worker)
            );
            let summary = api.runtime.worker(worker).unwrap();
            if commits == expected_commits
                && summary
                    .worker_state
                    .as_ref()
                    .is_some_and(|state| state.state == protocol::WorkerState::Idle)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("scripted Worker did not finish: expected_commits={expected_commits}, commits={:?}, receipts={:?}, worker={:?}",
        delivery_commits(api, worker), delivery_receipts(api, worker), api.runtime.worker(worker)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assignment_without_queue_reaches_real_worker_and_stopped_assign_survives_restore() {
    let dir = tempfile::tempdir().unwrap();
    let (api, execution, client) = delivery_real_fixture(dir.path(), false).await;
    let first = delivery_ticket(&api, "Assign alone is the delegation");
    let assigned = delivery_assign(&api, &first.id, "assign-before-start").await;
    let assignment = assigned["assignment"]["assignment_id"].as_str().unwrap();
    let initial = delivery_tracking(&api, &first.id, assignment);
    assert!(
        execution.attempts.lock().unwrap().is_empty(),
        "Assign must not auto-restore a stopped Orchestrator"
    );
    let worker = delivery_worker(start_attention_test_orchestrator(&api).await);
    delivery_wait_idle(&api, &worker, 1).await;
    assert_eq!(delivery_receipts(&api, &worker).len(), 1);
    assert!(delivery_commits(&api, &worker)[0].contains(first.resource_key.as_deref().unwrap()));
    assert_eq!(
        client.requests.lock().unwrap().len(),
        1,
        "real scripted model did not read the request"
    );
    assert_eq!(delivery_tracking(&api, &first.id, assignment), initial);
    delivery_stop(&api, &worker);
    let second = delivery_ticket(&api, "Assigned while stopped");
    let second_assigned = delivery_assign(&api, &second.id, "assign-while-stopped").await;
    let second_id = second_assigned["assignment"]["assignment_id"]
        .as_str()
        .unwrap();
    let second_initial = delivery_tracking(&api, &second.id, second_id);
    assert_eq!(execution.attempts.lock().unwrap().len(), 1);
    let restored = start_attention_test_orchestrator(&api).await;
    assert_eq!(restored.disposition, "restored");
    assert_eq!(delivery_worker(restored), worker);
    delivery_wait_idle(&api, &worker, 2).await;
    assert_eq!(delivery_receipts(&api, &worker).len(), 2);
    assert!(delivery_commits(&api, &worker)[1].contains(second.resource_key.as_deref().unwrap()));
    assert_eq!(
        delivery_tracking(&api, &second.id, second_id),
        second_initial
    );
    let accepted = execution.attempts.lock().unwrap().clone();
    delivery_scan(&api, &worker);
    delivery_stop(&api, &worker);
    assert_eq!(
        start_attention_test_orchestrator(&api).await.disposition,
        "restored"
    );
    delivery_scan(&api, &worker);
    assert_eq!(
        execution.attempts.lock().unwrap().as_slice(),
        accepted.as_slice(),
        "restoring the same Worker must retain durable acceptance, not generate new work"
    );
    assert_eq!(delivery_commits(&api, &worker).len(), 2);
    assert_eq!(delivery_receipts(&api, &worker).len(), 2);
    delivery_stop(&api, &worker);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assignment_real_worker_response_loss_restore_and_concurrent_scans_commit_once() {
    let dir = tempfile::tempdir().unwrap();
    let (mut api, execution, _) = delivery_real_fixture(dir.path(), true).await;
    let identity = RuntimeIdentityMaterial::generate(EMBEDDED_WORKER_RUNTIME_ID).unwrap();
    configure_runtime_request_auth(&mut api, &identity, EMBEDDED_WORKER_RUNTIME_ID);
    let ticket = delivery_ticket(&api, "Immutable lost-response payload");
    let assigned = delivery_assign(&api, &ticket.id, "assign-lost-response").await;
    let assignment = assigned["assignment"]["assignment_id"].as_str().unwrap();
    let initial = delivery_tracking(&api, &ticket.id, assignment);
    let worker = delivery_worker(start_attention_test_orchestrator(&api).await);
    delivery_wait_idle(&api, &worker, 1).await;
    let attempts = execution.attempts.lock().unwrap().clone();
    assert!(!attempts.is_empty());
    assert!(
        attempts.iter().all(|attempt| attempt == &attempts[0]),
        "uncertain delivery changed request id or payload: {attempts:?}"
    );
    assert_eq!(delivery_receipts(&api, &worker).len(), 1);
    // Change mutable rendering context while the acknowledgement is unknown.
    // A new self-report is not a new input and cannot change the frozen retry.
    seed_worker_source_member(&api, &worker.runtime_id, &worker.worker_id);
    delivery_self_report(
        &api,
        &identity,
        &worker,
        &initial,
        "waiting",
        "waiting-during-response-loss",
    )
    .await;
    let waiting = delivery_tracking(&api, &ticket.id, assignment);
    delivery_stop(&api, &worker);
    execution
        .lose_responses
        .store(false, std::sync::atomic::Ordering::SeqCst);
    browser_ticket_backend(&api)
        .unwrap()
        .edit_item(
            ticket.id.clone().into(),
            ticket::TicketItemEdit {
                title: Some("Title changed while acceptance was unknown".into()),
                ..Default::default()
            },
        )
        .unwrap();
    // Reopen while acceptance is still unknown. This intentionally discards the
    // old API's cached pending text, but not the durable input/Worker receipts.
    let config = api.config.clone();
    drop(api);
    tokio::task::yield_now().await;
    let reopened = WorkspaceApi::new_with_execution_backend(
        config.clone(),
        Arc::new(SqliteWorkspaceStore::open_for_test(&config.database_path).unwrap()),
        execution.clone(),
    )
    .await
    .unwrap();
    let restored = start_attention_test_orchestrator(&reopened).await;
    assert_eq!(restored.disposition, "restored");
    assert_eq!(delivery_worker(restored), worker);
    {
        let after_restart = execution.attempts.lock().unwrap();
        let same_request = after_restart
            .iter()
            .filter(|attempt| attempt.0 == attempts[0].0)
            .collect::<Vec<_>>();
        assert!(
            same_request.len() > attempts.len(),
            "unknown outcome was not retried after restore"
        );
        assert!(
            same_request.iter().all(|attempt| *attempt == &attempts[0]),
            "same request id changed payload after DB reopen/title/self-report change: {after_restart:?}"
        );
    }
    delivery_wait_idle(&reopened, &worker, 2).await;
    assert_eq!(delivery_receipts(&reopened, &worker).len(), 2);
    assert_eq!(
        delivery_commits(&reopened, &worker)
            .iter()
            .filter(|message| *message == &attempts[0].1)
            .count(),
        1,
        "accepted retry committed the old notification twice"
    );
    assert_eq!(
        delivery_tracking(&reopened, &ticket.id, assignment),
        waiting
    );
    let before = execution.attempts.lock().unwrap().len();
    browser_ticket_backend(&reopened)
        .unwrap()
        .add_event(
            ticket.id.clone().into(),
            ticket::NewTicketEvent::new(
                ticket::TicketEventKind::Comment,
                "External answer after server restart",
            ),
        )
        .unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(8));
    std::thread::scope(|scope| {
        let joins = (0..8)
            .map(|_| {
                let api = reopened.clone();
                let worker = worker.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    barrier.wait();
                    dispatch_orchestrator_assignment_attention(&api, &worker);
                })
            })
            .collect::<Vec<_>>();
        for join in joins {
            join.join().unwrap();
        }
    });
    delivery_wait_idle(&reopened, &worker, 3).await;
    assert_eq!(
        execution.attempts.lock().unwrap().len(),
        before + 1,
        "concurrent scans must share the committed reservation"
    );
    assert_eq!(delivery_receipts(&reopened, &worker).len(), 3);
    delivery_scan(&reopened, &worker);
    assert_eq!(execution.attempts.lock().unwrap().len(), before + 1);
    assert_eq!(
        delivery_tracking(&reopened, &ticket.id, assignment),
        waiting
    );
    delivery_stop(&reopened, &worker);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assignment_worker_replacement_receives_pending_input_without_recreating_delegation() {
    let dir = tempfile::tempdir().unwrap();
    let (api, execution, _) = delivery_real_fixture(dir.path(), false).await;
    let handled = delivery_ticket(&api, "Already received before replacement");
    let saved = delivery_assign(&api, &handled.id, "assign-before-replacement").await;
    let assignment = saved["assignment"]["assignment_id"].as_str().unwrap();
    let initial = delivery_tracking(&api, &handled.id, assignment);
    let old_worker = delivery_worker(start_attention_test_orchestrator(&api).await);
    delivery_wait_idle(&api, &old_worker, 1).await;
    delivery_stop(&api, &old_worker);
    // Delete only this isolated fixture's stopped identity. A new live Worker
    // has a new session; it is not a restore and cannot reuse old session receipts.
    api.runtime.delete_worker(&old_worker).unwrap();
    api.worker_projection
        .publish_ordered(|| {
            api.store
                .delete_worker_registry(TEST_WORKSPACE_ID, &old_worker)
                .map(|commit| ((), commit))
        })
        .unwrap();
    let pending = delivery_ticket(&api, "Pending input for replacement");
    let new_assign = delivery_assign(&api, &pending.id, "assign-pending-replacement").await;
    let pending_assignment = new_assign["assignment"]["assignment_id"].as_str().unwrap();
    let pending_initial = delivery_tracking(&api, &pending.id, pending_assignment);
    let started = start_attention_test_orchestrator(&api).await;
    assert_eq!(started.disposition, "created");
    let replacement = delivery_worker(started);
    assert_ne!(replacement, old_worker);
    delivery_wait_idle(&api, &replacement, 1).await;
    assert_eq!(delivery_receipts(&api, &replacement).len(), 1);
    let commits = delivery_commits(&api, &replacement);
    assert!(commits[0].contains(pending.resource_key.as_deref().unwrap()));
    assert!(
        !commits[0].contains(handled.resource_key.as_deref().unwrap()),
        "replacement must not execute acknowledged history"
    );
    assert_eq!(execution.attempts.lock().unwrap().len(), 2);
    assert_eq!(
        delivery_tracking(&api, &handled.id, assignment),
        initial,
        "Worker replacement must not reassign the stable principal"
    );
    assert_eq!(
        delivery_tracking(&api, &pending.id, pending_assignment),
        pending_initial
    );
    delivery_scan(&api, &replacement);
    assert_eq!(execution.attempts.lock().unwrap().len(), 2);
    delivery_stop(&api, &replacement);
}
