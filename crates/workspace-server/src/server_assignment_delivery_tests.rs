// Included as a child module of server::tests. These tests exercise committed
// Ticket activity + SQLite routing + HTTP + embedded Runtime boundaries. Only
// the model and explicit response-loss boundary are replaced in the real fixture.
// Run with cargo test -p yoi-workspace-server --lib assignment_delivery.
use super::*;
use session_store::{LogEntry, Store, WorkerSessionStore};
use worker_runtime::execution as delivery_execution;
include!("server_implementation_result_tests.rs");

#[tokio::test]
async fn notification_diagnostics_http_auth_scope_and_readonly_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let api = test_api(dir.path()).await;
    seed_test_api_token(api.store.as_ref(), TEST_WORKSPACE_ID);
    let ticket = delivery_ticket(&api, "Notification diagnostics auth fixture");
    delivery_assign(&api, ticket.id.as_str(), "diagnostics-assignment").await;
    let path = format!(
        "/api/w/{TEST_WORKSPACE_ID}/notification-diagnostics?ticket={}&limit=1",
        ticket.id.as_str()
    );
    let page = delivery_http(&api, "GET", &path, None, StatusCode::OK).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["model_processing_confirmed"], false);
    assert!(page["items"][0].get("content").is_none());
    let again = delivery_http(&api, "GET", &path, None, StatusCode::OK).await;
    assert_eq!(again, page);
    let response = build_router(api.clone())
        .oneshot(Request::builder().uri(&path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    delivery_http(
        &api,
        "GET",
        "/api/w/other-workspace/notification-diagnostics",
        None,
        StatusCode::NOT_FOUND,
    )
    .await;
    delivery_http(
        &api,
        "GET",
        &format!("/api/w/{TEST_WORKSPACE_ID}/notification-diagnostics?limit=101"),
        None,
        StatusCode::BAD_REQUEST,
    )
    .await;
}

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

async fn delivery_external_answer(api: &WorkspaceApi, ticket: &str, text: &str, key: &str) {
    let current = api
        .store
        .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, ticket)
        .unwrap()
        .remove(0);
    delivery_http(api,"POST",&format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignment-answer"),Some(json!({"assignment_id":current.assignment_id,"expected_last_event_id":current.last_event_id,"operation_key":key,"body":text})),StatusCode::OK).await;
}

fn delivery_commit_answer_without_scan(api: &WorkspaceApi, ticket: &str, text: &str, key: &str) {
    seed_test_api_token(api.store.as_ref(), "direct-answer");
    let current = api
        .store
        .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, ticket)
        .unwrap()
        .remove(0);
    api.store
        .answer_ticket_assignment(
            TEST_WORKSPACE_ID,
            ticket,
            &ticket::TicketAssignmentAnswer {
                assignment_id: current.assignment_id,
                expected_last_event_id: current.last_event_id.unwrap(),
                operation_key: key.into(),
                body: text.into(),
            },
            &crate::store::TicketOrchestratorAssignmentSourceContext::Account {
                account_id: "account-direct-answer".into(),
                user_id: "user-direct-answer".into(),
            },
            "fixture-account",
        )
        .unwrap();
}

fn delivery_comment_without_scan(api: &WorkspaceApi, ticket: &str, text: &str) {
    browser_ticket_backend(api)
        .unwrap()
        .add_event(
            ticket.to_owned().into(),
            ticket::NewTicketEvent::new(ticket::TicketEventKind::Comment, text),
        )
        .unwrap();
}

fn delivery_external_comment(api: &WorkspaceApi, ticket: &str, text: &str) {
    delivery_comment_without_scan(api, ticket, text);
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
    delivery_external_comment(&api, &ticket.id, "History-only annotation while waiting");
    assert!(
        execution.take_inputs().is_empty(),
        "ordinary waiting comment became fresh work"
    );
    delivery_external_answer(
        &api,
        &ticket.id,
        "Human answer: choose option B",
        "choose-B",
    )
    .await;
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

async fn delivery_public_assignment_snapshot(
    api: &WorkspaceApi,
    ticket: &str,
    assignment: &str,
) -> (Value, Value, Value) {
    let root = format!("/workspaces/{TEST_WORKSPACE_ID}/tickets/{ticket}");
    let detail = delivery_http(
        api,
        "GET",
        &format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}"),
        None,
        StatusCode::OK,
    )
    .await;
    let retained = delivery_http(
        api,
        "GET",
        &format!("{root}/assignment-tracking?assignment_id={assignment}"),
        None,
        StatusCode::OK,
    )
    .await;
    let history = delivery_http(
        api,
        "GET",
        &format!("{root}/assignment-tracking/history?assignment_id={assignment}&limit=100"),
        None,
        StatusCode::OK,
    )
    .await;
    (detail, retained, history)
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
    let (done_detail, done_public, done_history) =
        delivery_public_assignment_snapshot(&api, &ticket.id, assignment).await;
    let expected_done = serde_json::to_value(&done).unwrap();
    assert_eq!(
        expected_done["tracking"]["result"],
        "Returned options without closing"
    );
    assert_eq!(done_detail["state"], "open");
    assert_eq!(done_detail["assignment_tracking"], json!([expected_done]));
    assert_eq!(done_public, done_detail["assignment_tracking"]);
    let history_items = done_history["items"].as_array().unwrap();
    assert_eq!(history_items.len(), 2, "initial waiting and reported done");
    assert_eq!(history_items[0]["tracking"]["status"], "waiting");
    assert_eq!(
        history_items[1]["event_id"],
        done.last_event_id.as_deref().unwrap()
    );
    assert_eq!(history_items[1]["tracking"], expected_done["tracking"]);
    assert!(done_history["next_event_id"].is_null());
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
        let (detail, public, history) =
            delivery_public_assignment_snapshot(&api, &ticket.id, assignment).await;
        assert_eq!(
            detail["state"],
            if key == "close-return" {
                "closed"
            } else {
                "open"
            }
        );
        assert_eq!(
            detail["closure_history"].as_array().unwrap().len(),
            if key == "close-return" { 1 } else { 2 }
        );
        assert_eq!(
            detail["assignment_tracking"],
            done_detail["assignment_tracking"]
        );
        assert_eq!(public, done_public, "case decision rewrote public handling");
        assert_eq!(
            history, done_history,
            "case decision rewrote handling history"
        );
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
    let (detail, old_public, old_history) =
        delivery_public_assignment_snapshot(&api, &ticket.id, assignment).await;
    let (_, new_public, new_history) =
        delivery_public_assignment_snapshot(&api, &ticket.id, new_assignment).await;
    assert_eq!(detail["state"], "open");
    assert_eq!(detail["closure_history"].as_array().unwrap().len(), 2);
    assert_eq!(
        old_public,
        json!([serde_json::to_value(&retained).unwrap()])
    );
    assert_eq!(old_public[0]["tracking"], expected_done["tracking"]);
    assert_eq!(
        old_public[0]["last_event_id"],
        expected_done["last_event_id"]
    );
    assert_eq!(old_public[0]["release"]["action"], "replaced");
    assert_eq!(
        old_public[0]["release"]["replacement_assignment_id"],
        new_assignment
    );
    assert_eq!(old_public[0]["release"]["operation_key"], "reissue-return");
    assert_eq!(old_history, done_history, "reissue changed old history");
    assert_eq!(detail["assignment_tracking"], new_public);
    assert_eq!(new_public[0]["assignment_id"], new_assignment);
    assert_eq!(new_public[0]["current"], true);
    assert_eq!(new_public[0]["tracking"]["status"], "waiting");
    assert!(new_public[0]["tracking"]["result"].is_null());
    assert!(new_public[0]["release"].is_null());
    assert_eq!(new_history["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        new_history["items"][0]["event_id"],
        new_public[0]["last_event_id"]
    );
    // Follow the public cursor rather than assuming current-first ordering.
    let page_root = format!(
        "/workspaces/{TEST_WORKSPACE_ID}/tickets/{}/assignment-tracking/assignments",
        ticket.id
    );
    let first = delivery_http(
        &api,
        "GET",
        &format!("{page_root}?limit=1"),
        None,
        StatusCode::OK,
    )
    .await;
    let cursor = first["next_assignment_id"]
        .as_str()
        .expect("two assignments need a cursor");
    let second = delivery_http(
        &api,
        "GET",
        &format!("{page_root}?limit=1&after_assignment_id={cursor}"),
        None,
        StatusCode::OK,
    )
    .await;
    assert!(second["next_assignment_id"].is_null());
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    let rows = [&first["items"][0], &second["items"][0]];
    assert!(rows.contains(&&old_public[0]));
    assert!(rows.contains(&&new_public[0]));
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
        assert_ne!(
            ids[0], old_ids[0],
            "new assignment reused stale input identity"
        );
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
            delivery_self_report(&api, &identity, &worker, &current, "waiting", "wait-answer")
                .await;
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
                            &caller_api,
                            &caller_worker,
                            state,
                            pending,
                        );
                        if checks == 0 {
                            // The production scan has read an empty snapshot but
                            // still owns its reservation. Race a real HTTP commit.
                            entered_tx.send(()).unwrap();
                            release_rx
                                .recv_timeout(std::time::Duration::from_secs(5))
                                .unwrap();
                        }
                        checks += 1;
                    },
                );
                assert_eq!(checks, 2, "committed input must defer a scan");
            });
            entered_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            if answer {
                futures::executor::block_on(delivery_external_answer(
                    &api,
                    &ticket.id,
                    "Human answer: go ahead",
                    "racing-answer",
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
        assert!(
            inputs[0]
                .1
                .contains(ticket.resource_key.as_deref().unwrap())
        );
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
            &callback_api,
            &worker.worker_id.to_string(),
            Some(S::Running),
            S::Idle,
        );
    });
    delivery_assign(&api, &ticket.id, "assign-failed").await;
    assert_eq!(
        execution.take_inputs().len(),
        2,
        "idle must not add retries beyond the bounded send"
    );
    let ids = execution.take_input_request_ids();
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], ids[1]);
    assert_eq!(
        attention_pending(&api).len(),
        1,
        "failure must retain durable input"
    );
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
            restore_streams: None,
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
    async fn prepare_restore(
        &self,
        request: delivery_execution::WorkerExecutionRestoreRequest,
    ) -> std::result::Result<
        Box<dyn worker_runtime::worker_backend::RuntimeRestoreCandidate>,
        worker_runtime::worker_backend::RuntimeRestoreFailure,
    > {
        Ok(Box::new(DeliveryRestoreCandidate {
            inputs: Some((
                DeliveryWorkerFactory {
                    root: self.root.clone(),
                    client: self.client.clone(),
                },
                request.worker_ref.worker_id.to_string(),
            )),
        }))
    }
}

// Retain the same scripted client/receipt store without opening either store or
// starting a Controller until activation. Discard drops only these owned inputs.
struct DeliveryRestoreCandidate {
    inputs: Option<(DeliveryWorkerFactory, String)>,
}

#[async_trait::async_trait]
impl worker_runtime::worker_backend::RuntimeRestoreCandidate for DeliveryRestoreCandidate {
    fn initial_state(&self) -> protocol::WorkerStateSnapshot {
        protocol::WorkerStateSnapshot::initial()
    }

    async fn activate(
        &mut self,
    ) -> std::result::Result<
        worker_runtime::worker_backend::RuntimeWorkerController,
        worker_runtime::worker_backend::RuntimeRestoreFailure,
    > {
        let (factory, worker_id) = self.inputs.take().ok_or_else(|| {
            worker_runtime::worker_backend::RuntimeRestoreFailure::refused(
                worker_runtime::worker_backend::RuntimeRestoreStage::Controller,
                "delivery fixture candidate already consumed",
            )
        })?;
        factory.controller(worker_id, true).await.map_err(|_| {
            worker_runtime::worker_backend::RuntimeRestoreFailure::unproven(
                worker_runtime::worker_backend::RuntimeRestoreStage::Reconstruction,
                "delivery fixture reconstruction failed",
            )
        })
    }

    async fn discard(
        &mut self,
    ) -> std::result::Result<(), worker_runtime::worker_backend::RuntimeRestoreCleanupFailure> {
        self.inputs.take();
        Ok(())
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
    let _environment = restored_parent_control_tests::connected_restore_tempdir();
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
            let entries = delivery_session_entries(api, worker);
            let last_notification = entries.iter().rposition(|entry| matches!(
                entry,
                LogEntry::AnnotatedSystemItem { entry, .. }
                    if matches!(entry.item, session_store::SystemItem::Notification { .. })
            ));
            let last_completed = entries.iter().rposition(|entry| matches!(entry, LogEntry::RunCompleted { .. }));
            // Runtime acceptance and the saved notification precede processing.
            // Initial Idle can therefore be observed before any model request.
            // Require a committed run outcome after the latest input, not a
            // guessed delay or Worker liveness as proof of consumption.
            let processed = last_notification.zip(last_completed)
                .is_some_and(|(input, completed)| completed > input);
            let summary = api.runtime.worker(worker).unwrap();
            if commits == expected_commits
                && processed
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
    let unknown = api
        .store
        .notification_diagnostics(
            TEST_WORKSPACE_ID,
            &server_api::NotificationDiagnosticsQuery {
                assignment_id: Some(assignment.into()),
                ..Default::default()
            },
        )
        .unwrap()
        .items
        .into_iter()
        .find(|item| item.stage == server_api::NotificationDiagnosticStage::AcceptanceUnknown)
        .expect(
            "saved unknown acceptance must remain visible while Runtime already has its receipt",
        );
    assert!(!unknown.model_processing_confirmed);
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
    let evidence = delivery_http(
        &reopened,
        "GET",
        &format!(
            "/api/w/{TEST_WORKSPACE_ID}/notification-diagnostics?source_id={}",
            unknown.source_id
        ),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(evidence["items"][0]["stage"], "runtime_accepted");
    assert_eq!(
        evidence["items"][0]["delivery_id"],
        json!(unknown.delivery_id)
    );
    assert_eq!(evidence["items"][0]["model_processing_confirmed"], false);
    assert_eq!(evidence["items"][0]["retry_candidate"], false);
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
    delivery_commit_answer_without_scan(
        &reopened,
        &ticket.id,
        "External answer after server restart",
        "answer-after-restart",
    );
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

fn delivery_assign_worker(
    api: &WorkspaceApi,
    ticket: &str,
    worker: &RuntimeWorkerRef,
    assignment: &str,
    previous: Option<&str>,
) -> TicketWorkerAssignmentRecord {
    let record = TicketWorkerAssignmentRecord {
        workspace_id: TEST_WORKSPACE_ID.into(),
        ticket_id: ticket.into(),
        assignment_id: assignment.into(),
        worker: worker.clone(),
        assigned_by: "fixture-account".into(),
        assigned_at: TEST_CREATED_AT.into(),
    };
    api.store
        .set_current_ticket_worker_assignment(
            &record,
            previous,
            &format!("{assignment}-event"),
            &format!("{assignment}-operation"),
            previous.is_some(),
        )
        .unwrap();
    record
}

fn delivery_worker_pending(api: &WorkspaceApi) -> Vec<crate::store::TicketWorkerNotification> {
    api.store
        .list_pending_ticket_worker_notifications(TEST_WORKSPACE_ID, 20)
        .unwrap()
}

async fn delivery_settle_real_worker_assignment(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> usize {
    let initial = delivery_worker_pending(api).len();
    recover_ticket_notifications(api);
    if initial > 0 {
        delivery_wait_idle(api, worker, initial).await;
    }
    assert!(delivery_worker_pending(api).is_empty());
    initial
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_notification_missing_callback_is_recovered_to_real_worker_once() {
    let dir = tempfile::tempdir().unwrap();
    let (api, execution, _) = delivery_real_fixture(dir.path(), false).await;
    let worker = spawn_ticket_check_source(&api, "notification-recipient");
    let ticket = delivery_ticket(&api, "Recover committed Worker activity");
    let assignment = delivery_assign_worker(&api, &ticket.id, &worker, "worker-owner", None);
    let initial = delivery_settle_real_worker_assignment(&api, &worker).await;
    let before = execution.attempts.lock().unwrap().len();

    // Commit the actual domain event but deliberately omit the mutation callback.
    delivery_comment_without_scan(&api, &ticket.id, "A human supplied useful context");
    assert_eq!(execution.attempts.lock().unwrap().len(), before);
    let pending = delivery_worker_pending(&api);
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].worker, worker);
    recover_ticket_notifications(&api);
    delivery_wait_idle(&api, &worker, initial + 1).await;
    let attempts = execution.attempts.lock().unwrap().clone();
    assert_eq!(attempts.len(), before + 1);
    assert_eq!(attempts[before].0, pending[0].request_id);
    assert!(
        attempts[before]
            .1
            .contains(ticket.resource_key.as_deref().unwrap())
    );
    assert!(attempts[before].1.contains(&pending[0].input_event_id));
    assert_eq!(delivery_receipts(&api, &worker).len(), initial + 1);
    assert!(delivery_worker_pending(&api).is_empty());
    for _ in 0..3 {
        recover_ticket_notifications(&api);
    }
    assert_eq!(*execution.attempts.lock().unwrap(), attempts);
    assert_eq!(
        api.store
            .get_current_ticket_worker_assignment(TEST_WORKSPACE_ID, &ticket.id)
            .unwrap(),
        Some(assignment),
        "attention acceptance must not replace assignment authority"
    );
    assert_eq!(
        api.authority.ticket(&ticket.id).unwrap().state.as_str(),
        "open"
    );
    delivery_stop(&api, &worker);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_notification_response_loss_retries_same_request_without_duplicate_acceptance() {
    let dir = tempfile::tempdir().unwrap();
    let (api, execution, _) = delivery_real_fixture(dir.path(), false).await;
    let worker = spawn_ticket_check_source(&api, "lost-response-recipient");
    let ticket = delivery_ticket(&api, "Worker response loss");
    delivery_assign_worker(&api, &ticket.id, &worker, "worker-owner", None);
    let initial = delivery_settle_real_worker_assignment(&api, &worker).await;
    let before = execution.attempts.lock().unwrap().len();
    execution
        .lose_responses
        .store(true, std::sync::atomic::Ordering::SeqCst);
    delivery_comment_without_scan(&api, &ticket.id, "Preserve this notification identity");
    let pending = delivery_worker_pending(&api);
    assert_eq!(pending.len(), 1);
    recover_ticket_notifications(&api);
    delivery_wait_idle(&api, &worker, initial + 1).await;
    let accepted = execution.attempts.lock().unwrap()[before].clone();
    assert_eq!(accepted.0, pending[0].request_id);
    let failure: String = api.config_store.with_conn(|conn| {
        Ok(conn.query_row("SELECT code FROM notification_failures WHERE workspace_id=?1 AND delivery_id=?2 ORDER BY sequence DESC LIMIT 1", rusqlite::params![TEST_WORKSPACE_ID, pending[0].delivery_id], |row| row.get(0))?)
    }).unwrap();
    assert_eq!(
        failure, "acceptance_unknown",
        "response loss is not a confirmed rejection"
    );
    assert_eq!(delivery_receipts(&api, &worker).len(), initial + 1);
    assert_eq!(
        delivery_worker_pending(&api),
        pending,
        "unknown ACK was treated as success"
    );

    // Reopen the durable Store while keeping the same real Runtime/session. A
    // later domain edit must not change the frozen text of the uncertain request.
    browser_ticket_backend(&api)
        .unwrap()
        .edit_item(
            ticket.id.clone().into(),
            ticket::TicketItemEdit {
                title: Some("Title changed after Worker acceptance".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let mut recovered = api.clone();
    recovered.store =
        Arc::new(SqliteWorkspaceStore::open_for_test(&api.config.database_path).unwrap());
    execution
        .lose_responses
        .store(false, std::sync::atomic::Ordering::SeqCst);
    recover_ticket_notifications(&recovered);
    let attempts = execution.attempts.lock().unwrap().clone();
    let retries = attempts
        .iter()
        .filter(|attempt| attempt.0 == accepted.0)
        .collect::<Vec<_>>();
    assert!(
        retries.len() >= 2,
        "uncertain Runtime acceptance was not retried"
    );
    assert!(
        retries.iter().all(|attempt| **attempt == accepted),
        "retry changed frozen ID/body"
    );
    let distinct = attempts
        .iter()
        .map(|attempt| &attempt.0)
        .collect::<std::collections::HashSet<_>>()
        .len();
    delivery_wait_idle(&recovered, &worker, distinct).await;
    assert_eq!(delivery_receipts(&recovered, &worker).len(), distinct);
    assert_eq!(
        delivery_commits(&recovered, &worker)
            .iter()
            .filter(|body| **body == accepted.1)
            .count(),
        1
    );
    assert!(delivery_worker_pending(&recovered).is_empty());
    recover_ticket_notifications(&recovered);
    assert_eq!(*execution.attempts.lock().unwrap(), attempts);
    delivery_stop(&recovered, &worker);
}

#[tokio::test]
async fn worker_notification_stopped_recipient_does_not_block_other_recipient() {
    let dir = tempfile::tempdir().unwrap();
    let (api, execution, orchestrator, _) = delivery_recording_fixture(dir.path()).await;
    let worker = spawn_ticket_check_source(&api, "stopped-notification-recipient");
    let ticket = delivery_ticket(&api, "One input with independent recipients");
    delivery_assign(&api, &ticket.id, "assign-orchestrator").await;
    delivery_assign_worker(&api, &ticket.id, &worker, "worker-owner", None);
    recover_ticket_notifications(&api);
    assert!(delivery_worker_pending(&api).is_empty());
    execution.take_inputs();
    execution.take_input_request_ids();
    delivery_stop(&api, &worker);
    delivery_commit_answer_without_scan(
        &api,
        &ticket.id,
        "Continue with this answer",
        "answer-both",
    );
    let pending = delivery_worker_pending(&api);
    assert_eq!(pending.len(), 1);
    recover_ticket_notifications(&api);
    let inputs = execution.take_inputs();
    assert_eq!(
        inputs.len(),
        1,
        "stopped Worker must not block Orchestrator delivery"
    );
    assert_eq!(inputs[0].0.worker_id.to_string(), orchestrator.worker_id);
    assert!(
        inputs[0]
            .1
            .contains(ticket.resource_key.as_deref().unwrap())
    );
    assert!(attention_pending(&api).is_empty());
    assert_eq!(
        delivery_worker_pending(&api),
        pending,
        "other recipient ACK consumed the failed delivery"
    );
    recover_ticket_notifications(&api);
    assert!(
        execution.take_inputs().is_empty(),
        "successful recipient was sent again"
    );
    delivery_stop(&api, &orchestrator);
}

#[tokio::test]
async fn worker_notification_reassignment_never_forwards_old_pending_input() {
    let dir = tempfile::tempdir().unwrap();
    let (api, execution, orchestrator, _) = delivery_recording_fixture(dir.path()).await;
    let old_worker = spawn_ticket_check_source(&api, "old-notification-recipient");
    let replacement = spawn_ticket_check_source(&api, "new-notification-recipient");
    let ticket = delivery_ticket(&api, "Do not forward old authority");
    delivery_assign_worker(&api, &ticket.id, &old_worker, "old-owner", None);
    recover_ticket_notifications(&api);
    execution.take_inputs();
    execution.take_input_request_ids();
    delivery_comment_without_scan(&api, &ticket.id, "Activity belonging to the old owner");
    let pending = delivery_worker_pending(&api);
    assert_eq!(pending.len(), 1);
    execution.reject_inputs("response outcome unknown");
    recover_ticket_notifications(&api);
    let failed = execution.take_inputs();
    assert!(!failed.is_empty());
    assert!(
        failed
            .iter()
            .all(|input| input.0.worker_id.to_string() == old_worker.worker_id)
    );
    assert!(
        execution
            .take_input_request_ids()
            .iter()
            .all(|id| id.as_deref() == Some(pending[0].request_id.as_str()))
    );

    let current = delivery_assign_worker(
        &api,
        &ticket.id,
        &replacement,
        "new-owner",
        Some("old-owner"),
    );
    *execution.input_failure.lock().unwrap() = None;
    delivery_comment_without_scan(&api, &ticket.id, "Activity belonging to the new owner");
    recover_ticket_notifications(&api);
    let inputs = execution.take_inputs();
    assert!(!inputs.is_empty(), "new owner's current activity was lost");
    assert!(
        inputs
            .iter()
            .all(|input| input.0.worker_id.to_string() == replacement.worker_id)
    );
    assert!(
        inputs
            .iter()
            .all(|input| !input.1.contains(&pending[0].input_event_id)),
        "old activity was forwarded to the replacement"
    );
    assert!(
        execution
            .take_input_request_ids()
            .iter()
            .all(|id| id.as_deref() != Some(pending[0].request_id.as_str()))
    );
    assert!(delivery_worker_pending(&api).is_empty());
    assert_eq!(
        api.store
            .get_current_ticket_worker_assignment(TEST_WORKSPACE_ID, &ticket.id)
            .unwrap(),
        Some(current)
    );
    recover_ticket_notifications(&api);
    assert!(execution.take_inputs().is_empty());
    delivery_stop(&api, &old_worker);
    delivery_stop(&api, &replacement);
    delivery_stop(&api, &orchestrator);
}

async fn delivery_drain_host_with_paused_time(api: &WorkspaceApi) {
    let executor = api.host_executors.host().unwrap();
    let drain = executor.drain();
    tokio::pin!(drain);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let std::task::Poll::Ready(result) = futures::poll!(drain.as_mut()) {
            result.unwrap();
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "notification host round did not finish"
        );
        // Keep a runnable future while the real host executor finishes: Tokio
        // must not auto-advance the paused retry clock to infer completion.
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn worker_notification_periodic_recovery_runs_without_runtime_subscription() {
    let dir = tempfile::tempdir().unwrap();
    let (mut api, execution, orchestrator, _) = delivery_recording_fixture(dir.path()).await;
    let worker = spawn_ticket_check_source(&api, "periodic-notification-recipient");
    let ticket = delivery_ticket(&api, "Recover without subscription or callback");
    delivery_assign_worker(&api, &ticket.id, &worker, "worker-owner", None);
    recover_ticket_notifications(&api);
    execution.take_inputs();
    execution.take_input_request_ids();
    api.runtime_subscription_broker = RuntimeSubscriptionBroker::new(TEST_WORKSPACE_ID);
    assert!(
        api.runtime_subscription_broker
            .subscribe(
                EMBEDDED_WORKER_RUNTIME_ID,
                protocol::subscription::EventSubscriptionSelector::RuntimeWorkers,
            )
            .is_err()
    );

    // Poll the production hook itself, not a timer or recovery stand-in. Keeping
    // its future local also guarantees cancellation on assertion failure.
    delivery_comment_without_scan(&api, &ticket.id, "Missed startup callback");
    let hook = run_orchestrator_turn_end_hook(api.clone());
    tokio::pin!(hook);
    assert!(futures::poll!(hook.as_mut()).is_pending());
    delivery_drain_host_with_paused_time(&api).await;
    assert_eq!(
        execution.take_inputs().len(),
        1,
        "startup round did not recover"
    );
    assert!(delivery_worker_pending(&api).is_empty());
    assert!(futures::poll!(hook.as_mut()).is_pending());

    delivery_comment_without_scan(&api, &ticket.id, "Missed callback after startup");
    tokio::time::advance(std::time::Duration::from_secs(29)).await;
    assert!(futures::poll!(hook.as_mut()).is_pending());
    delivery_drain_host_with_paused_time(&api).await;
    assert!(
        execution.take_inputs().is_empty(),
        "recovery retried before the bounded interval"
    );
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    assert!(futures::poll!(hook.as_mut()).is_pending());
    delivery_drain_host_with_paused_time(&api).await;
    let inputs = execution.take_inputs();
    assert_eq!(
        inputs.len(),
        1,
        "periodic recovery required a subscription or mutation callback"
    );
    assert_eq!(inputs[0].0.worker_id.to_string(), worker.worker_id);
    assert!(delivery_worker_pending(&api).is_empty());
    assert!(api.runtime_subscription_broker.runtime_ids().is_empty());
    delivery_stop(&api, &worker);
    delivery_stop(&api, &orchestrator);
}

async fn delivery_signed_item_edit(
    api: &WorkspaceApi,
    identity: &RuntimeIdentityMaterial,
    worker: &RuntimeWorkerRef,
    ticket: &str,
    edit: Value,
) -> ticket::Ticket {
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/item");
    let response = build_router(api.clone())
        .oneshot(runtime_source_request(
            identity,
            Some(&worker.worker_id),
            "PATCH",
            &path,
            serde_json::to_vec(&edit).unwrap(),
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
    let edited: ticket::Ticket = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        edited,
        browser_ticket_backend(api)
            .unwrap()
            .show(ticket.to_owned().into())
            .unwrap()
    );
    edited
}

#[tokio::test]
async fn notification_item_edit_worker_title_only_self_edit_does_not_notify_itself() {
    let dir = tempfile::tempdir().unwrap();
    let (api, execution, orchestrator, identity) = delivery_recording_fixture(dir.path()).await;
    let worker = spawn_ticket_check_source(&api, "self-title-editor");
    seed_worker_source_member(&api, &worker.runtime_id, &worker.worker_id);
    let ticket = delivery_ticket(&api, "Worker will clarify its own title");
    delivery_assign_worker(&api, &ticket.id, &worker, "self-title-owner", None);
    recover_ticket_notifications(&api);
    execution.take_inputs();
    execution.take_input_request_ids();
    let before = browser_ticket_backend(&api)
        .unwrap()
        .show(ticket.id.clone().into())
        .unwrap();

    let edited = delivery_signed_item_edit(
        &api,
        &identity,
        &worker,
        &ticket.id,
        json!({"title":"Clarified by the assigned Worker"}),
    )
    .await;
    assert_eq!(edited.meta.title, "Clarified by the assigned Worker");
    assert_eq!(edited.document.body, before.document.body);
    assert_eq!(edited.meta.targets, before.meta.targets);
    for _ in 0..3 {
        recover_ticket_notifications(&api);
    }
    let inputs = execution.take_inputs();
    delivery_stop(&api, &worker);
    delivery_stop(&api, &orchestrator);
    assert!(
        inputs
            .iter()
            .all(|(recipient, _)| recipient.worker_id.to_string() != worker.worker_id),
        "authenticated title-only self-edit became fresh Worker input"
    );
    assert!(delivery_worker_pending(&api).is_empty());
}

#[tokio::test]
async fn notification_item_edit_orchestrator_initial_body_self_edit_does_not_reenter_delegation() {
    let dir = tempfile::tempdir().unwrap();
    let (api, execution, orchestrator, identity) = delivery_recording_fixture(dir.path()).await;
    let ticket = delivery_ticket(&api, "Orchestrator clarifies the initial post");
    let assigned = delivery_assign(&api, &ticket.id, "assign-self-body").await;
    let assignment = assigned["assignment"]["assignment_id"].as_str().unwrap();
    let initial = delivery_tracking(&api, &ticket.id, assignment);
    execution.take_inputs();
    execution.take_input_request_ids();
    let before = browser_ticket_backend(&api)
        .unwrap()
        .show(ticket.id.clone().into())
        .unwrap();

    let edited = delivery_signed_item_edit(
        &api,
        &identity,
        &orchestrator,
        &ticket.id,
        json!({"body":"Clarified requirements written by the current Orchestrator"}),
    )
    .await;
    assert_eq!(
        edited.document.body.as_str(),
        "Clarified requirements written by the current Orchestrator"
    );
    assert_eq!(edited.meta.title, before.meta.title);
    assert_eq!(
        edited.events[0].edited_by.as_deref(),
        Some(
            format!(
                "worker:{}/{}",
                orchestrator.runtime_id, orchestrator.worker_id
            )
            .as_str()
        )
    );
    for _ in 0..3 {
        recover_ticket_notifications(&api);
    }
    let inputs = execution.take_inputs();
    assert_eq!(delivery_tracking(&api, &ticket.id, assignment), initial);
    delivery_stop(&api, &orchestrator);
    assert!(
        inputs
            .iter()
            .all(|(recipient, _)| recipient.worker_id.to_string() != orchestrator.worker_id),
        "workspace_agent principal did not recognize its authenticated Orchestrator self-edit"
    );
    assert!(attention_pending(&api).is_empty());
}

#[tokio::test]
async fn notification_item_edit_external_targets_only_change_notifies_existing_recipients_once() {
    let dir = tempfile::tempdir().unwrap();
    let (api, execution, orchestrator, _) = delivery_recording_fixture(dir.path()).await;
    let worker = spawn_ticket_check_source(&api, "target-change-recipient");
    let ticket = delivery_ticket(&api, "External target change must not disappear");
    delivery_assign(&api, &ticket.id, "assign-targets").await;
    delivery_assign_worker(&api, &ticket.id, &worker, "targets-owner", None);
    recover_ticket_notifications(&api);
    execution.take_inputs();
    execution.take_input_request_ids();
    let before = browser_ticket_backend(&api)
        .unwrap()
        .show(ticket.id.clone().into())
        .unwrap();
    let targets = vec![test_ticket_target(
        "test-repository",
        "develop",
        TicketTargetAccess::ReadWrite,
    )];
    let result = delivery_http(
        &api,
        "PATCH",
        &format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{}/item", ticket.id),
        Some(json!({"targets":{"action":"set","targets":targets}})),
        StatusCode::OK,
    )
    .await;
    let edited: ticket::Ticket = serde_json::from_value(result).unwrap();
    assert_eq!(
        edited,
        browser_ticket_backend(&api)
            .unwrap()
            .show(ticket.id.clone().into())
            .unwrap()
    );
    assert_eq!(edited.meta.targets, targets);
    assert_eq!(edited.meta.title, before.meta.title);
    assert_eq!(edited.document.body, before.document.body);
    recover_ticket_notifications(&api);
    let inputs = execution.take_inputs();
    let ids = execution.take_input_request_ids();
    recover_ticket_notifications(&api);
    let repeats = execution.take_inputs();
    delivery_stop(&api, &worker);
    delivery_stop(&api, &orchestrator);
    for recipient in [&orchestrator, &worker] {
        assert_eq!(
            inputs
                .iter()
                .filter(|(target, _)| target.worker_id.to_string() == recipient.worker_id)
                .count(),
            1,
            "targets-only external edit did not reach {} exactly once: {inputs:?}",
            recipient.worker_id
        );
    }
    assert_eq!(ids.len(), 2);
    assert!(ids.iter().all(Option::is_some));
    assert_ne!(ids[0], ids[1]);
    assert!(repeats.is_empty(), "targets edit was delivered repeatedly");
    assert!(delivery_worker_pending(&api).is_empty());
    assert!(attention_pending(&api).is_empty());
}

#[tokio::test]
async fn catalog_shutdown_stops_notification_hook_and_fences_later_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let (api, execution, orchestrator, _) = delivery_recording_fixture(dir.path()).await;
    let ticket = delivery_ticket(&api, "Pending at server shutdown");
    delivery_assign(&api, &ticket.id, "shutdown-assignment").await;
    execution.take_inputs();
    delivery_commit_answer_without_scan(
        &api,
        &ticket.id,
        "Retain for the next server",
        "shutdown-answer",
    );
    let catalog = WorkspaceServerApi::new(api.config.clone(), api.store.clone());
    catalog
        .apis
        .lock()
        .await
        .insert(TEST_WORKSPACE_ID.into(), api.clone());
    let hook = tokio::spawn(run_orchestrator_turn_end_hook(api.clone()));
    catalog
        .hook_handles
        .lock()
        .await
        .insert(TEST_WORKSPACE_ID.into(), hook.abort_handle());
    catalog.shutdown_feature_storage().await.unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), hook)
            .await
            .unwrap()
            .unwrap_err()
            .is_cancelled()
    );
    assert!(catalog.hook_handles.lock().await.is_empty());
    execution.take_inputs();
    recover_ticket_notifications(&api);
    dispatch_orchestrator_assignment_attention(&api, &orchestrator);
    assert!(
        execution.take_inputs().is_empty(),
        "stopped server dispatched a saved notification"
    );
    assert_eq!(
        attention_pending(&api).len(),
        1,
        "shutdown discarded durable work"
    );
    delivery_stop(&api, &orchestrator);
}

include!("server_job_notification_tests.rs");
