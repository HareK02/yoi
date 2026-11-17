//! Connected Ticket delegation: current positive scenarios live in the child modules.
//! Retained resource-scoped authority must not mint key-only all-target grants.
//! The external model is scripted; public Tools, signed routing and stores are real.
use super::*;
#[path = "subject_ticket_normal_connected_tests.rs"]
mod ordinary_subject;
#[path = "subject_ticket_mr_connected_tests.rs"]
mod ordinary_subject_mr;
use agen::llm_client::{
    event::{Event, StopReason},
    types::{ContentPart, Item},
};
use worker_runtime::worker_backend::RuntimeWorkerFactory;

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
    let source = ConfigCommitRequest {
        base_digest: state.snapshot.digest.clone(),
        entrypoints: state.contract.entrypoints.clone(),
        changes: vec![config_source::ConfigTreeChange::Update {
            path: main.path.clone(),
            expected_digest: main.content_digest.clone(),
            content: content.into(),
        }],
    };
    let owner = test_owner_actor().account_id;
    let registry = api.permission_registry.read().unwrap();
    api.config_store
        .adopt_configuration(
            TEST_WORKSPACE_ID,
            &owner,
            &crate::config_adoption::ConfigurationAdoptionRequest {
                operation_id: Uuid::now_v7().to_string(),
                expected_head: api
                    .config_store
                    .configuration_head(TEST_WORKSPACE_ID, &owner)
                    .unwrap(),
                expected_catalog_digest: crate::config_adoption::catalog_digest(registry.host())
                    .unwrap(),
                source,
            },
            api.config_schema_registry.compose().unwrap(),
            registry.host(),
        )
        .unwrap();
}

#[derive(Clone)]
struct LegacyScopeModel {
    completed: tokio::sync::mpsc::Sender<()>,
}
#[async_trait::async_trait]
impl agen::llm_client::LlmClient for LegacyScopeModel {
    fn clone_boxed(&self) -> Box<dyn agen::llm_client::LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        request: agen::llm_client::Request,
    ) -> std::result::Result<agen::llm_client::client::ResponseStream, agen::llm_client::ClientError>
    {
        let events = if let Some(input) = config(&request, "legacy-scope-probe:") {
            if let Some((error, content)) = tool_result(&request, "legacy-spawn") {
                assert!(
                    error,
                    "legacy resource bounds must not become all-target keys"
                );
                let failure: Value = serde_json::from_str(&content).unwrap();
                assert_eq!(failure["result"]["status"], 403, "{failure}");
                self.completed.try_send(()).unwrap();
                done()
            } else {
                call(&request, "legacy-spawn", "WorkerSpawn", input)
            }
        } else {
            done()
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn public_ticket_spawn_refuses_legacy_scoped_authority_without_broadening_or_allocating() {
    // New key-only delegation cannot preserve old Ticket/path restrictions.
    // Current positive two-Subject execution and MR flows live in the child modules;
    // this retained-policy case must refuse rather than reintroduce scoped issuance.
    let (completed, mut completion) = tokio::sync::mpsc::channel(4);
    let h = ConnectedWorkerHarness::start(LegacyScopeModel { completed }).await;
    install_public_ticket_profiles(&h.api);
    let api = &h.api;
    let subjects = open_subjektiv_store(api).unwrap();
    let subject = subjects
        .create_subject(crate::subjektiv::SubjectRole::new("retained-scoped-origin").unwrap())
        .unwrap()
        .id;
    let ticket = browser_ticket_backend(api)
        .unwrap()
        .create(NewTicket::new("Legacy scoped creation refusal"))
        .unwrap()
        .id;
    let mut execution = vec![
        collection("worker.create"),
        ticket_permission(&ticket, "ticket.assignment.assign"),
    ];
    execution.extend(
        [
            "session.record",
            "memory.resident.read",
            "memory.resident.supply",
            "journal.startup.read",
            "journal.startup.supply",
        ]
        .into_iter()
        .map(|name| subject_permission(&subject, name)),
    );
    let mut delegation = file_permissions("retained-workdir");
    delegation.push(ticket_permission(&ticket, "ticket.read"));
    let policy = seed_test_subject_legacy_constraints(api, &subject, execution, delegation);
    set_test_subject_policy_as_owner(api, &subject, policy.execution, policy.delegation);
    let before_policy = subjects.current_subject_policy(&subject).unwrap().unwrap();
    assert!(before_policy.legacy_constraints.is_some());
    let (status, source) = h.human(reqwest::Method::POST, &format!("/api/w/{TEST_WORKSPACE_ID}/workers"),
        json!({"runtime_id":CONNECTED_RUNTIME,"display_name":"Retained scoped source","profile":"project:public-ticket-origin",
            "initial_submit":[],"workdir_attachments":[],"feature_connections":{"subjektiv":{"subject_id":subject}}})).await;
    assert_eq!(status, StatusCode::OK, "{source}");
    let origin = RuntimeWorkerRef::new(
        source["runtime_id"].as_str().unwrap(),
        source["worker_id"].as_str().unwrap(),
    );
    let (status, assigned) = h.human(reqwest::Method::PUT, &format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/assignments/orchestrator"),
        json!({"operation_id":"legacy-probe-assign","expected_assignment_id":null,"principal":{"kind":"subject","subject_id":subject}})).await;
    assert_eq!(status, StatusCode::OK, "{assigned}");
    let assignment = assigned["assignment"]["assignment_id"].clone();
    let authorization = test_worker_operation_authorization(&[("ticket", &["ticket.read"])]);
    let spawn = json!({"runtime_id":CONNECTED_RUNTIME,"profile":"project:public-ticket-editor",
        "operation_id":"legacy-public-spawn","display_name":"Must not allocate",
        "ticket_id":ticket,"orchestrator_assignment_id":assignment,
        "initial_submit":[],"authorization":authorization});
    let counts = || {
        api.config_store
            .with_conn(|conn| {
                Ok([
                    "worker_create_reservations",
                    "core_worker_execution_grants",
                    "ticket_assignment_operations",
                ]
                .map(|table| {
                    conn.query_row(
                        &format!("SELECT count(*) FROM {table} WHERE workspace_id=?1"),
                        [TEST_WORKSPACE_ID],
                        |r| r.get::<_, i64>(0),
                    )
                    .unwrap()
                }))
            })
            .unwrap()
    };
    let before = counts();
    let (status, body) = h
        .human(
            reqwest::Method::POST,
            &format!(
                "/api/w/{TEST_WORKSPACE_ID}/runtimes/{}/workers/{}/input",
                origin.runtime_id, origin.worker_id
            ),
            json!({"kind":"notify","content":format!("legacy-scope-probe:{spawn}")}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    tokio::time::timeout(std::time::Duration::from_secs(45), completion.recv())
        .await
        .unwrap()
        .unwrap();
    // Observe the specific refusal at its authoritative HTTP boundary as well as
    // the sanitized public Tool status; no response text is rewritten for the model.
    let (status, body) = connected_authorization_post(&h.backend_url, &h.identity, &origin,
        &format!("/api/w/{TEST_WORKSPACE_ID}/worker-control/workers"),
        &json!({"runtime_id":CONNECTED_RUNTIME,"profile":"project:public-ticket-editor","display_name":"Must not allocate",
            "ticket_assignment":{"ticket_id":ticket,"operation_id":"legacy-backend-spawn","orchestrator_assignment_id":assignment},
            "control_operation_id":"legacy-backend-spawn","authorization":authorization,
            "initial_submit":[],"workdir_attachments":[],"feature_connections":{}})).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("legacy scoped authority"),
        "{body}"
    );
    assert_eq!(counts(), before);
    assert_eq!(
        subjects.current_subject_policy(&subject).unwrap().unwrap(),
        before_policy
    );
    assert_eq!(h.runtime.list_workers().unwrap().len(), 1);
    assert!(
        api.store
            .get_current_ticket_worker_assignment(TEST_WORKSPACE_ID, &ticket)
            .unwrap()
            .is_none()
    );
}
