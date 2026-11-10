//! Retained106 -> normal migration/reopen -> ordinary Subject public continuation.
//! Only the historical database is seeded with SQL. Subject creation, canonical
//! policy adoption, body startup, reassignment and evaluation use production APIs.
//! No fixture Profile, Ticket-ID policy patch, receipt rewrite or schema relabel.
use super::*;
use agen::llm_client::{
    event::{Event, StopReason},
    types::{ContentPart, Item},
};
use rusqlite::{Connection, params};
use std::time::Duration;
use worker_runtime::worker_backend::RuntimeWorkerFactory;

const TICKET: &str = "0192f0e8-4d84-7d6e-a846-000000000001";
const OLD_ASSIGNMENT: &str = "retained106-orchestrator";
const OLD_EVENT: &str = "retained106-assigned";
const OLD_WORKER: &str = "retained106-worker";
const OLD_RESULT: &str = "retained106-saved-result";
const RESULT_BODY: &str = "Retained implementation evidence must be evaluated, not executed again.";
const MARKER: &str = "evaluate-retained106:";
const LIMIT: Duration = Duration::from_secs(60);

// Include rowids and exact persisted JSON, not merely equivalent decoded values.
// The current pointer is deliberately excluded: explicit reassignment changes it.
fn retained_evidence(conn: &Connection) -> Vec<Vec<Vec<rusqlite::types::Value>>> {
    [
        "SELECT rowid,* FROM __yoi_schema_migrations WHERE version<=106 ORDER BY rowid",
        "SELECT rowid,* FROM ticket_schema_migrations WHERE version<=13 ORDER BY rowid",
        "SELECT rowid,workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,runtime_id,worker_id,assigned_by,assigned_at,tracking_json,last_tracking_event_id,tracking_updated_at FROM ticket_worker_assignments WHERE assignment_id='retained106-orchestrator' ORDER BY rowid",
        "SELECT rowid,* FROM ticket_worker_assignment_events WHERE event_id='retained106-assigned' ORDER BY rowid",
        "SELECT rowid,* FROM ticket_assignment_operations WHERE operation_id='retained106-assign' ORDER BY rowid",
        "SELECT rowid,* FROM typed_ticket_assignment_tracking_history WHERE assignment_id='retained106-orchestrator' ORDER BY rowid",
        "SELECT rowid,* FROM typed_ticket_assignment_tracking_operations WHERE operation_key='retained106-report' ORDER BY rowid",
        "SELECT rowid,* FROM ticket_assignment_deliveries WHERE assignment_id='retained106-orchestrator' ORDER BY rowid",
        "SELECT rowid,* FROM ticket_assignment_delivery_requests WHERE delivery_id='retained106-delivery' ORDER BY rowid",
        "SELECT rowid,* FROM ticket_implementation_results WHERE result_id='retained106-saved-result' ORDER BY rowid",
    ].into_iter().map(|sql| {
        let mut statement = conn.prepare(sql).unwrap();
        let columns = statement.column_count();
        let rows = statement.query_map([], |row| {
            (0..columns).map(|index| row.get(index)).collect::<rusqlite::Result<Vec<rusqlite::types::Value>>>()
        }).unwrap();
        rows.collect::<rusqlite::Result<Vec<_>>>().unwrap()
    }).collect()
}

fn prepare_legacy(config: &ServerConfig) -> Vec<Vec<Vec<rusqlite::types::Value>>> {
    if let Some(parent) = config.database_path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    crate::store::prepare_retained_test_schema(&config.database_path, 106);
    let conn = Connection::open(&config.database_path).unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT max(version) FROM __yoi_schema_migrations",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        106
    );
    assert_eq!(
        conn.query_row(
            "SELECT max(version) FROM ticket_schema_migrations",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        13
    );
    let owner = test_owner_actor().account_id;
    conn.execute("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES(?1,'user','owner','Owner','1','2')", [&owner]).unwrap();
    conn.execute("INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES(?1,?2,'Workspace','active','1','2')", params![TEST_WORKSPACE_ID, owner]).unwrap();
    conn.execute("INSERT INTO worker_registry(workspace_id,runtime_id,worker_id,display_name,retention_state,created_at,updated_at) VALUES(?1,?2,?3,'Historical singleton','normal','1','2')", params![TEST_WORKSPACE_ID, CONNECTED_RUNTIME, OLD_WORKER]).unwrap();
    conn.execute("INSERT INTO worker_singleton_owners(workspace_id,singleton_key,runtime_id,worker_id,generation,created_at,updated_at) VALUES(?1,'workspace-orchestrator',?2,?3,1,'1','2')", params![TEST_WORKSPACE_ID, CONNECTED_RUNTIME, OLD_WORKER]).unwrap();
    conn.execute("INSERT INTO typed_tickets(workspace_id,ticket_id,slug,title,status,kind,priority,workflow_state,workflow_state_explicit) VALUES(?1,?2,'retained106-public-continuation','Evaluate retained evidence','open','task','P2','open',1)", params![TEST_WORKSPACE_ID, TICKET]).unwrap();
    conn.execute("INSERT INTO workspace_resource_key_counters(workspace_id,resource_kind,next_sequence) VALUES(?1,'ticket',2)",[TEST_WORKSPACE_ID]).unwrap();
    conn.execute("INSERT INTO workspace_resource_keys(workspace_id,resource_kind,resource_id,sequence,resource_key,allocated_at) VALUES(?1,'ticket',?2,1,'T-1','2025-01-01T00:00:00Z')",params![TEST_WORKSPACE_ID,TICKET]).unwrap();
    conn.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,event_ref,kind,body) VALUES(?1,?2,-1,'retained106-initial','comment','Evaluate retained implementation evidence; do not run the old input again.')", params![TEST_WORKSPACE_ID, TICKET]).unwrap();
    conn.execute("INSERT INTO ticket_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,assigned_by,assigned_at) VALUES(?1,?2,?3,'orchestrator','workspace_agent','workspace-orchestrator','historical owner','2025-01-01T00:00:00Z')", params![TEST_WORKSPACE_ID, TICKET, OLD_ASSIGNMENT]).unwrap();
    let tracking = json!({"status":"inprogress","waiting_reason":null,"progress":"Implementation saved before upgrade","next_action":"Evaluate saved result","result":null});
    conn.execute("INSERT INTO ticket_current_worker_assignments(workspace_id,ticket_id,role,assignment_id,principal_kind,principal_id,updated_at) VALUES(?1,?2,'orchestrator',?3,'workspace_agent','workspace-orchestrator','2025-01-01T00:00:00Z')", params![TEST_WORKSPACE_ID, TICKET, OLD_ASSIGNMENT]).unwrap();
    // The real retained triggers initialize history, input origin and notification source.
    conn.execute("INSERT INTO ticket_worker_assignment_events(workspace_id,ticket_id,role,event_id,action,assignment_id,actor,created_at,operation_id) VALUES(?1,?2,'orchestrator',?3,'assigned',?4,'historical owner','2025-01-01T00:00:00Z','retained106-assign')", params![TEST_WORKSPACE_ID, TICKET, OLD_EVENT, OLD_ASSIGNMENT]).unwrap();
    conn.execute("INSERT INTO ticket_assignment_operations(workspace_id,operation_id,action,ticket_id,role,principal_kind,principal_id,assignment_id,created_at,request_fingerprint) VALUES(?1,'retained106-assign','assign',?2,'orchestrator','workspace_agent','workspace-orchestrator',?3,'2025-01-01T00:00:00Z','retained106-immutable-fingerprint')", params![TEST_WORKSPACE_ID, TICKET, OLD_ASSIGNMENT]).unwrap();
    let update = json!({"assignment_id":OLD_ASSIGNMENT,"expected_last_event_id":OLD_EVENT,"operation_key":"retained106-report","tracking":tracking});
    let source = json!({"actor":"historical body","runtime_id":CONNECTED_RUNTIME,"worker_id":OLD_WORKER,"agent_key":"workspace-orchestrator","authentication_binding_id":"retained106-binding"});
    conn.execute("INSERT INTO typed_ticket_assignment_tracking_history(workspace_id,ticket_id,assignment_id,event_id,operation_key,tracking_json,source_context_json,at) VALUES(?1,?2,?3,'retained106-report-event','retained106-report',?4,?5,'2025-01-01T00:00:01Z')", params![TEST_WORKSPACE_ID, TICKET, OLD_ASSIGNMENT, tracking.to_string(), source.to_string()]).unwrap();
    conn.execute("UPDATE ticket_worker_assignments SET tracking_json=?2,last_tracking_event_id='retained106-report-event',tracking_updated_at='2025-01-01T00:00:01Z' WHERE assignment_id=?1", params![OLD_ASSIGNMENT, tracking.to_string()]).unwrap();
    let receipt = json!({"workspace_id":TEST_WORKSPACE_ID,"ticket_id":TICKET,"assignment_id":OLD_ASSIGNMENT,"role":"orchestrator","principal":{"kind":"workspace_agent","agent_key":"workspace-orchestrator"},"assigned_by":"historical owner","assigned_at":"2025-01-01T00:00:00Z","release":null,"current":true,"tracking":tracking,"last_event_id":"retained106-report-event","updated_at":"2025-01-01T00:00:01Z","migration_diagnostic":null});
    // Validate typed payloads rather than inheriting the migration unit fixture's placeholders.
    serde_json::from_value::<ticket::TicketAssignmentTrackingUpdate>(update.clone()).unwrap();
    serde_json::from_value::<ticket::TicketAssignmentTrackingProjection>(receipt.clone()).unwrap();
    conn.execute("INSERT INTO typed_ticket_assignment_tracking_operations(workspace_id,operation_key,ticket_id,input_json,source_context_json,result_json) VALUES(?1,'retained106-report',?2,?3,?4,?5)", params![TEST_WORKSPACE_ID, TICKET, update.to_string(), source.to_string(), receipt.to_string()]).unwrap();
    let pending = json!({"ticket_id":TICKET,"assignment_id":OLD_ASSIGNMENT,"input_event_id":format!("assignment-event:{OLD_EVENT}"),"delivery_id":"retained106-delivery","request_id":"retained106-delivery","waiting_reason":null,"next_action":"Evaluate saved result"});
    serde_json::from_value::<crate::store::TicketAssignmentDelivery>(pending.clone()).unwrap();
    conn.execute("INSERT INTO ticket_assignment_deliveries(workspace_id,assignment_id,input_event_id,delivery_id,payload_json,reserved_source_json) VALUES(?1,?2,?3,'retained106-delivery',?4,?5)", params![TEST_WORKSPACE_ID, OLD_ASSIGNMENT, format!("assignment-event:{OLD_EVENT}"), pending.to_string(), source.to_string()]).unwrap();
    conn.execute("INSERT INTO ticket_assignment_delivery_requests(workspace_id,delivery_id,content) VALUES(?1,'retained106-delivery','OLD INPUT MUST NEVER BE RESUBMITTED')", [TEST_WORKSPACE_ID]).unwrap();
    let result = json!({"operation_key":"retained106-result-operation","kind":"result","body":RESULT_BODY,"merge_request":null});
    serde_json::from_value::<server_api::TicketImplementationResultRequest>(result.clone())
        .unwrap();
    conn.execute("INSERT INTO ticket_implementation_results(workspace_id,ticket_id,operation_key,result_id,runtime_id,worker_id,request_json,created_at) VALUES(?1,?2,'retained106-result-operation',?3,?4,?5,?6,'2025-01-01T00:00:01Z')", params![TEST_WORKSPACE_ID, TICKET, OLD_RESULT, CONNECTED_RUNTIME, OLD_WORKER, result.to_string()]).unwrap();
    retained_evidence(&conn)
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
            let text = content.as_ref().unwrap_or(summary);
            assert!(!is_error, "public {id} failed: {text}");
            Some(serde_json::from_str(text).expect("public tool JSON"))
        }
        _ => None,
    })
}
fn call(request: &agen::llm_client::Request, id: &str, name: &str, input: Value) -> Vec<Event> {
    assert!(
        request.tools.iter().any(|tool| tool.name == name),
        "builtin body must expose {name}"
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
        Event::text_delta(0, "retained evidence evaluated"),
        Event::text_block_stop(0, Some(StopReason::EndTurn)),
    ]
}
#[derive(Clone)]
struct MigrationModel {
    completed: tokio::sync::mpsc::Sender<Value>,
    reported: Arc<std::sync::atomic::AtomicBool>,
}
impl MigrationModel {
    async fn evaluate(&self, request: &agen::llm_client::Request, input: &Value) -> Vec<Event> {
        // Only reads and a current-assignment report are scripted below; there is
        // no implementation spawn or authority refresh during this evaluation.
        let Some(ticket) = result(request, "migration-show") else {
            return call(
                request,
                "migration-show",
                "ShowTicket",
                json!({"id":TICKET}),
            );
        };
        assert!(ticket["content_digest"].is_string());
        assert_eq!(
            ticket["initial_post"]["body"],
            "Evaluate retained implementation evidence; do not run the old input again."
        );
        let Some(tracking) = result(request, "migration-current") else {
            return call(
                request,
                "migration-current",
                "TicketAssignmentTrackingQuery",
                json!({"ticket":TICKET}),
            );
        };
        let rows = tracking.as_array().expect("current assignment rows");
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row["principal"]["subject_id"], input["subject"]);
        assert_ne!(row["assignment_id"], OLD_ASSIGNMENT);
        let Some(history) = result(request, "migration-history") else {
            return call(
                request,
                "migration-history",
                "TicketAssignmentTrackingQuery",
                json!({"ticket":TICKET,"assignment_id":OLD_ASSIGNMENT}),
            );
        };
        // Exact historical row is readable, but never a source of new authority.
        let history = history.as_array().expect("historical assignment rows");
        assert_eq!(history.len(), 1);
        let history = &history[0];
        assert_eq!(history["principal"]["kind"], "workspace_agent");
        assert_eq!(history["current"], false);
        assert_eq!(history["tracking"]["status"], "inprogress");
        let Some(saved) = result(request, "migration-result") else {
            return call(
                request,
                "migration-result",
                "TicketImplementationResultQuery",
                json!({"ticket":TICKET,"result_id":OLD_RESULT}),
            );
        };
        assert_eq!(saved["ticket_id"], TICKET);
        assert_eq!(saved["request"]["body"], RESULT_BODY);
        assert_eq!(
            saved["request"]["operation_key"],
            "retained106-result-operation"
        );
        let Some(updated) = result(request, "migration-evaluated") else {
            return call(
                request,
                "migration-evaluated",
                "TicketAssignmentTrackingUpdate",
                json!({
                    "ticket":TICKET,"assignment_id":row["assignment_id"],"expected_last_event_id":row["last_event_id"],
                    "operation_key":"ordinary-subject-evaluated-retained106",
                    "tracking":{"status":"done","waiting_reason":null,"progress":null,"next_action":null,"result":format!("Evaluated saved evidence {OLD_RESULT}")}
                }),
            );
        };
        assert_eq!(updated["assignment_id"], row["assignment_id"]);
        assert_eq!(updated["tracking"]["status"], "done");
        if !self
            .reported
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            self.completed.send(updated).await.unwrap();
        }
        done()
    }
}
#[async_trait::async_trait]
impl agen::llm_client::LlmClient for MigrationModel {
    fn clone_boxed(&self) -> Box<dyn agen::llm_client::LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        request: agen::llm_client::Request,
    ) -> std::result::Result<agen::llm_client::client::ResponseStream, agen::llm_client::ClientError>
    {
        let input = request.items.iter().find_map(|item| {
            let Item::Message { content, .. } = item else {
                return None;
            };
            content.iter().find_map(|part| match part {
                ContentPart::Text { text } => text.find(MARKER).map(|offset| {
                    serde_json::Deserializer::from_str(&text[offset + MARKER.len()..])
                        .into_iter::<Value>()
                        .next()
                        .unwrap()
                        .unwrap()
                }),
                _ => None,
            })
        });
        let events = match input {
            Some(input) => self.evaluate(&request, &input).await,
            None => done(),
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}
async fn ok(h: &ConnectedWorkerHarness, method: &str, path: &str, body: Value) -> Value {
    let (status, result) = h.human(method.parse().unwrap(), path, body).await;
    assert_eq!(status, StatusCode::OK, "{method} {path}: {result}");
    result
}
fn worker_count(h: &ConnectedWorkerHarness) -> usize {
    h.api
        .runtime
        .list_workers_for_runtime(CONNECTED_RUNTIME, 100)
        .unwrap()
        .items
        .len()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn retained106_explicit_transfer_to_normal_subject_evaluates_saved_result_without_restore() {
    crate::restore_test_environment::require_restore_test_environment();
    let (completed, mut completion) = tokio::sync::mpsc::channel(1);
    let baseline = Arc::new(Mutex::new(None));
    let capture = baseline.clone();
    let h = ConnectedWorkerHarness::start_with_database(
        MigrationModel {
            completed,
            reported: Default::default(),
        },
        move |config| {
            *capture.lock().unwrap() = Some(prepare_legacy(config));
        },
    )
    .await;
    let baseline = baseline.lock().unwrap().take().unwrap();
    assert_eq!(
        worker_count(&h),
        0,
        "migration must not restore the historical singleton"
    );
    let db = &h.api.config.database_path;
    let assert_preserved = || {
        let conn = Connection::open(db).unwrap();
        assert_eq!(retained_evidence(&conn), baseline);
        assert_eq!(
            conn.query_row(
                "SELECT max(version) FROM __yoi_schema_migrations",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            129
        );
        assert_eq!(
            conn.query_row(
                "SELECT max(version) FROM ticket_schema_migrations",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            17
        );
        assert_eq!(conn.query_row("SELECT count(*) FROM ticket_assignment_delivery_acknowledgements WHERE delivery_id='retained106-delivery'", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        assert_eq!(conn.query_row("SELECT count(*) FROM ticket_implementation_results WHERE workspace_id=?1 AND ticket_id=?2", params![TEST_WORKSPACE_ID, TICKET], |r| r.get::<_, i64>(0)).unwrap(), 1,
            "evaluating a retained result must not generate another implementation result");
    };
    assert_preserved();
    // Reopen via the production owner, not a second DDL installer or version edit.
    drop(crate::store::SqliteWorkspaceStore::open_for_test(db).unwrap());
    assert_preserved();
    assert_eq!(worker_count(&h), 0);
    let old = h
        .api
        .store
        .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, TICKET)
        .unwrap()
        .remove(0);
    assert_eq!(old.assignment_id, OLD_ASSIGNMENT);
    assert!(
        old.migration_diagnostic.is_some(),
        "legacy responsibility must be explicitly nonoperational"
    );
    assert_eq!(
        old.tracking.unwrap().status,
        ticket::TicketAssignmentTrackingStatus::Inprogress
    );

    h.api
        .runtime
        .unregister_if_idle(EMBEDDED_WORKER_RUNTIME_ID, 100)
        .unwrap();
    let (status, subject) = h.human(reqwest::Method::POST,
        &format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects"),
        json!({"operation_id":"ordinary-migration-subject","role":"Evaluate retained evidence","behavior_md":""})).await;
    assert_eq!(status, StatusCode::CREATED, "{subject}");
    let subject = subject["id"].as_str().unwrap().to_owned();
    // Operation keys/sets only; no per-Ticket IDs and no post-start authority patch.
    super::chat_tests::adopt_subject_policy(&h.api, &h.owner, &subject,
        r#"{ sets = ["conversation", "work-coordination"]; namespaces = [{ namespace = "ticket"; keys = ["ticket.read", "ticket.assignment.update"]; }]; }"#,
        "{}").await;
    let body_path = format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{subject}/body");
    let start = json!({"request_id":"ordinary-migration-start","expected_current":null,"expected_observation_token":null});
    let started = ok(&h, "POST", &format!("{body_path}/start"), start.clone()).await;
    assert_eq!(started["disposition"], "accepted", "{started}");
    assert_eq!(
        started["body"]["observation"]["worker"]["profile"],
        "builtin:async-chat"
    );
    assert_ne!(started["body"]["current"]["worker_id"], OLD_WORKER);
    assert_eq!(worker_count(&h), 1);
    let replay = ok(&h, "POST", &format!("{body_path}/start"), start).await;
    assert_eq!(replay["body"]["current"], started["body"]["current"]);
    assert_eq!(
        worker_count(&h),
        1,
        "same public body start must not allocate twice"
    );
    assert_preserved();
    // Starting a Subject must not silently convert the retained current principal.
    assert_eq!(
        h.api
            .store
            .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, TICKET)
            .unwrap()[0]
            .assignment_id,
        OLD_ASSIGNMENT
    );
    let assignment_path =
        format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{TICKET}/assignments/orchestrator");
    let assign = json!({"operation_id":"explicit-retained106-transfer","expected_assignment_id":OLD_ASSIGNMENT,"principal":{"kind":"subject","subject_id":subject}});
    let assigned = ok(&h, "PUT", &assignment_path, assign.clone()).await;
    assert_ne!(assigned["assignment"]["assignment_id"], OLD_ASSIGNMENT);
    let replay = ok(&h, "PUT", &assignment_path, assign).await;
    assert_eq!(
        replay, assigned,
        "explicit transfer replay is the same saved receipt"
    );
    assert_eq!(
        worker_count(&h),
        1,
        "assignment must not start a second Worker"
    );

    // A legacy actor/principal cannot obtain fresh responsibility via the public API.
    let (status, rejected) = h.human(reqwest::Method::PUT, &assignment_path,
        json!({"operation_id":"reject-legacy-reassignment","expected_assignment_id":assigned["assignment"]["assignment_id"],"principal":{"kind":"workspace_agent","agent_key":"workspace-orchestrator"}})).await;
    assert!(
        status.is_client_error(),
        "legacy principal must not become operational: {status} {rejected}"
    );
    // Signed transport is real, but neither a historical actor nor a saved
    // receipt grants authority. No runtime/grant/credential row is fabricated.
    let tracking_path =
        format!("/workspaces/{TEST_WORKSPACE_ID}/tickets/{TICKET}/assignment-tracking");
    let receipt_input = json!({"assignment_id":OLD_ASSIGNMENT,"expected_last_event_id":OLD_EVENT,
        "operation_key":"retained106-report","tracking":{"status":"inprogress","waiting_reason":null,
        "progress":"Implementation saved before upgrade","next_action":"Evaluate saved result","result":null}});
    let current_body = RuntimeWorkerRef::new(
        CONNECTED_RUNTIME,
        started["body"]["current"]["worker_id"].as_str().unwrap(),
    );
    let (status, rejected) = connected_authorization_post(
        &h.backend_url,
        &h.identity,
        &current_body,
        &tracking_path,
        &receipt_input,
    )
    .await;
    assert!(
        status.is_client_error(),
        "new body must not inherit the legacy receipt: {status} {rejected}"
    );
    let mut old_actor_input = receipt_input;
    old_actor_input["operation_key"] = json!("legacy-actor-new-report");
    let (status, rejected) = connected_authorization_post(
        &h.backend_url,
        &h.identity,
        &RuntimeWorkerRef::new(CONNECTED_RUNTIME, OLD_WORKER),
        &tracking_path,
        &old_actor_input,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "historical actor has no new authority: {rejected}"
    );
    assert_preserved();
    let conversation = ok(
        &h,
        "POST",
        &format!("/api/w/{TEST_WORKSPACE_ID}/chat/conversations"),
        json!({"operation_id":"migration-evaluation-chat","subject_id":subject}),
    )
    .await;
    let conversation = conversation["id"].as_str().unwrap();
    let sent = ok(&h, "POST", &format!("/api/w/{TEST_WORKSPACE_ID}/chat/conversations/{conversation}/messages"),
        json!({"operation_id":"evaluate-migrated-result","text":format!("{MARKER}{}", json!({"subject":subject}))})).await;
    assert_eq!(sent["delivery"]["state"], "accepted", "{sent}");
    let evaluated = tokio::time::timeout(LIMIT, completion.recv())
        .await
        .expect("current ordinary body must run public Show/Tracking/ResultQuery/Update")
        .unwrap();
    assert_eq!(
        evaluated["assignment_id"],
        assigned["assignment"]["assignment_id"]
    );
    assert_eq!(
        evaluated["tracking"]["result"],
        format!("Evaluated saved evidence {OLD_RESULT}")
    );
    assert_eq!(
        worker_count(&h),
        1,
        "saved evidence must not trigger implementation restart"
    );
    let snapshot = h
        .factory
        .retained_session_snapshot(
            &worker_runtime::identity::WorkerRef::try_from(&current_body).unwrap(),
        )
        .unwrap();
    assert!(!snapshot.snapshot.entries.iter().any(|entry| matches!(
        &entry.data,
        protocol::SessionSnapshotEntryData::SystemItem { item_kind, content, .. }
            if item_kind == "notification" && content.contains("OLD INPUT MUST NEVER BE RESUBMITTED")
    )), "historical pending input must not be delivered to the new body");
    assert_preserved();
    drop(crate::store::SqliteWorkspaceStore::open_for_test(db).unwrap());
    assert_preserved();
    let current = h
        .api
        .store
        .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, TICKET)
        .unwrap()
        .remove(0);
    assert_eq!(
        current.assignment_id,
        evaluated["assignment_id"].as_str().unwrap()
    );
    assert_eq!(
        current.tracking.unwrap().status,
        ticket::TicketAssignmentTrackingStatus::Done
    );
    assert_eq!(
        h.api.authority.ticket(TICKET).unwrap().state,
        server_api::BrowserTicketState::Open
    );
}
