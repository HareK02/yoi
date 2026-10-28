// Tools/WIP -> per-request Runtime signature -> production router/SQLite.
// Reuses the workdirless tracking fixture; its Runtime records execution rather
// than creating sessions. No LLM, repository, live service or delivery E2E here.
// Run: cargo test -p yoi-workspace-server --lib tools_backend_tests
use agen::tool::{Tool, ToolError, ToolExecutionContext, ToolOutput};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use worker::feature::builtin::ticket::{TicketFeature, TicketFeatureAccess};
use worker::{
    WorkspaceClient, WorkspaceClientError, WorkspaceRequest, WorkspaceRequestMethod,
    WorkspaceResponse,
};

struct TicketRouterClient {
    api: WorkspaceApi,
    identity: Arc<RuntimeIdentityMaterial>,
    source: Mutex<RuntimeWorkerRef>,
    runtime: tokio::runtime::Handle,
    lose_next_response: AtomicBool,
    requests: Mutex<Vec<(String, u16, String)>>,
}
impl std::fmt::Debug for TicketRouterClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SignedTicketRouterClient")
    }
}
impl WorkspaceClient for TicketRouterClient {
    fn workspace_id(&self) -> Option<&str> {
        Some(TEST_WORKSPACE_ID)
    }
    fn kind(&self) -> &str {
        "signed-ticket-router"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn execute(
        &self,
        request: WorkspaceRequest,
    ) -> std::result::Result<WorkspaceResponse, WorkspaceClientError> {
        let method = match request.method {
            WorkspaceRequestMethod::Get => "GET",
            WorkspaceRequestMethod::Post => "POST",
            WorkspaceRequestMethod::Put => "PUT",
            WorkspaceRequestMethod::Patch => "PATCH",
            WorkspaceRequestMethod::Delete => "DELETE",
        };
        let source = self.source.lock().unwrap().clone();
        let signed = runtime_source_request(
            &self.identity,
            Some(&source.worker_id),
            method,
            &request.path,
            request.body.unwrap_or_default().into_bytes(),
        );
        // Test tool dispatch runs on spawn_blocking, so this sync transport
        // can drive the router without nesting runtimes on an async thread.
        self.runtime.block_on(async {
            let response = build_router(self.api.clone())
                .oneshot(signed)
                .await
                .map_err(|e| WorkspaceClientError::Request(e.to_string()))?;
            let status = response.status().as_u16();
            let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
                .await
                .map_err(|e| WorkspaceClientError::Request(e.to_string()))?;
            let body = String::from_utf8(bytes.to_vec()).unwrap();
            let mutation = request.path.ends_with("/assignment-answer")
                || request.path.ends_with("/implementation-results");
            self.requests
                .lock()
                .unwrap()
                .push((request.path, status, body.clone()));
            if method == "POST"
                && mutation
                && (200..300).contains(&status)
                && self.lose_next_response.swap(false, Ordering::SeqCst)
            {
                return Err(WorkspaceClientError::Request(
                    "response lost after commit; outcome unknown".into(),
                ));
            }
            Ok(WorkspaceResponse { status, body })
        })
    }
}

#[derive(Clone)]
struct NoTicketLlm;
#[async_trait::async_trait]
impl agen::llm_client::LlmClient for NoTicketLlm {
    fn clone_boxed(&self) -> Box<dyn agen::llm_client::LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        _: agen::llm_client::Request,
    ) -> std::result::Result<agen::llm_client::client::ResponseStream, agen::llm_client::ClientError>
    {
        panic!("Ticket tools fixture must not call an LLM")
    }
}
async fn execute_tool(
    tool: Arc<dyn Tool>,
    input: serde_json::Value,
    context: ToolExecutionContext,
) -> std::result::Result<serde_json::Value, ToolError> {
    tokio::task::spawn_blocking(move || {
        futures::executor::block_on(tool.execute(&input.to_string(), context)).map(output_json)
    })
    .await
    .unwrap()
}
fn output_json(output: ToolOutput) -> serde_json::Value {
    serde_json::from_str(output.content.as_deref().unwrap_or(&output.summary)).unwrap()
}
struct TicketSurface {
    tools: BTreeMap<String, Arc<dyn Tool>>,
    wip: bool,
}
impl TicketSurface {
    async fn new(root: &Path, client: Arc<TicketRouterClient>, wip: bool) -> Self {
        Self::with_access(root, client, wip, TicketFeatureAccess::workflow()).await
    }
    async fn with_access(
        root: &Path,
        client: Arc<dyn WorkspaceClient>,
        wip: bool,
        access: TicketFeatureAccess,
    ) -> Self {
        let manifest = worker::WorkerManifest::from_toml("[worker]\nname=\"ticket-tools\"\n[model]\nscheme=\"anthropic\"\nmodel_id=\"unused\"\n[engine]\n[scope]\nallow=[]\n").unwrap();
        let engine =
            agen::Engine::<_, agen::state::Mutable, worker::SessionHistoryMetadata>::new_annotated(
                NoTicketLlm,
            );
        let mut host = worker::Worker::new(
            manifest,
            engine,
            session_store::FsStore::new(root.join("sessions")).unwrap(),
            worker::WorkerWorkspaceContext::with_client(
                Some(worker::WorkspaceId::new(TEST_WORKSPACE_ID).unwrap()),
                client.clone(),
            ),
            worker::WorkerFilesystemAuthority::None,
            manifest::Scope::empty(),
        )
        .await
        .unwrap();
        let report = host.install_features(
            worker::feature::FeatureRegistryBuilder::new()
                .with_module(TicketFeature::new(client.clone(), access)),
        );
        assert!(!report.has_errors());
        let handle = host.engine().tool_server_handle();
        handle.flush_pending();
        let tools = access
            .tool_names()
            .into_iter()
            .map(|name| (name.into(), handle.get_tool(name).unwrap().1))
            .collect();
        if !wip {
            return Self { tools, wip };
        }
        let mut mounts = worker::wip::WipMountRegistry::new();
        let route = mounts.allocate_namespace("ticket", "tickets").unwrap();
        worker::feature::builtin::ticket::mount_workspace_http_ticket_wip(
            &mut mounts,
            client,
            access,
            None,
            &route,
        )
        .unwrap();
        let mut engine = agen::Engine::<_, agen::state::Mutable, ()>::new_annotated(NoTicketLlm);
        worker::wip::install_wip_mode_with_mounts(&mut engine, None, "ticket-tools".into(), mounts)
            .unwrap();
        let handle = engine.tool_server_handle();
        handle.flush_pending();
        Self {
            tools: ["Inspect", "Invoke"]
                .into_iter()
                .map(|name| (name.into(), handle.get_tool(name).unwrap().1))
                .collect(),
            wip,
        }
    }
    async fn call(
        &self,
        name: &str,
        mut input: serde_json::Value,
        context: ToolExecutionContext,
    ) -> std::result::Result<serde_json::Value, ToolError> {
        if !self.wip {
            return execute_tool(self.tools[name].clone(), input, context).await;
        }
        let reference_field = if name == "ShowTicket" { "id" } else { "ticket" };
        let path = format!("/tickets/{}", input[reference_field].as_str().unwrap());
        input.as_object_mut().unwrap().remove(reference_field);
        match name {
            "ShowTicket" => {
                let mut page = serde_json::Map::new();
                for (legacy, native) in [("event_limit", "limit"), ("event_cursor", "cursor")] {
                    if let Some(value) = input.as_object_mut().unwrap().remove(legacy) {
                        page.insert(native.into(), value);
                    }
                }
                if !page.is_empty() {
                    input["page"] = page.into();
                }
            }
            "TicketAssignmentAnswer" => {
                let fields = input.as_object_mut().unwrap();
                let assignment_id = fields.remove("assignment_id").unwrap();
                let expected_last_event_id = fields.remove("expected_last_event_id").unwrap();
                input["observed"] = serde_json::json!({"assignment_id":assignment_id,"expected_last_event_id":expected_last_event_id});
            }
            _ => {}
        }
        // Observe the real item representation before invoking a mutation. Tracking
        // reads alone do not establish the Ticket WIP object validator. Explicit
        // refresh also reconciles stale observations after an unknown result;
        // it does not resend the operation or manufacture an observed CAS value.
        let inspected = execute_tool(
            self.tools["Inspect"].clone(),
            serde_json::json!({"path":path,"refresh":true}),
            ToolExecutionContext::direct(),
        )
        .await?;
        if name == "ShowTicket" {
            return execute_tool(self.tools["Invoke"].clone(), serde_json::json!({"path":path,"interface":inspected["interfaces"][0]["reference"],"operation":"read","arguments":input}), context).await;
        }
        execute_tool(self.tools["Invoke"].clone(), serde_json::json!({"path":path,"interface":inspected["interfaces"][0]["reference"],"operation":"read","arguments":{}}), ToolExecutionContext::direct()).await?;
        let inspected = execute_tool(
            self.tools["Inspect"].clone(),
            serde_json::json!({"path":path,"refresh":true}),
            ToolExecutionContext::direct(),
        )
        .await?;
        let operation = match name {
            "TicketAssignmentTrackingQuery" => "assignment_tracking_query",
            "TicketAssignmentAnswer" => "assignment_answer",
            "TicketImplementationResult" => "implementation_result",
            "TicketImplementationResultQuery" => "implementation_result_query",
            _ => unreachable!(),
        };
        execute_tool(self.tools["Invoke"].clone(), serde_json::json!({"path":path,"interface":inspected["interfaces"][0]["reference"],"operation":operation,"arguments":input}), context).await
    }
}
fn ticket_client(
    api: &WorkspaceApi,
    identity: RuntimeIdentityMaterial,
    source: &RuntimeWorkerRef,
) -> Arc<TicketRouterClient> {
    Arc::new(TicketRouterClient {
        api: api.clone(),
        identity: Arc::new(identity),
        source: Mutex::new(source.clone()),
        runtime: tokio::runtime::Handle::current(),
        lose_next_response: AtomicBool::new(false),
        requests: Mutex::new(Vec::new()),
    })
}
fn context(call: &str) -> ToolExecutionContext {
    ToolExecutionContext::new(call, "ticket-tools-batch", 0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ticket_public_roundtrip_observed_answer_replays_without_changing_waiting_delegation() {
    for wip in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let (api, runtime, identity, source, ticket) = tracking_fixture(dir.path()).await;
        let current = assign_tracking(&api, &identity, &source, &ticket).await;
        let input_count = runtime.input_requests.lock().unwrap().len();
        let client = ticket_client(&api, identity, &source);
        let surface = TicketSurface::new(dir.path(), client.clone(), wip).await;
        let observed = surface
            .call(
                "TicketAssignmentTrackingQuery",
                serde_json::json!({"ticket":ticket}),
                context("read"),
            )
            .await
            .unwrap();
        assert_eq!(observed[0]["assignment_id"], current.assignment_id);
        assert_eq!(
            observed[0]["last_event_id"],
            serde_json::to_value(&current.last_event_id).unwrap()
        );
        assert_eq!(observed[0]["tracking"]["status"], "waiting");
        let input = serde_json::json!({"ticket":ticket,"assignment_id":observed[0]["assignment_id"],"expected_last_event_id":observed[0]["last_event_id"],"operation_key":"answer-observed","body":"Use the bounded option"});
        client.lose_next_response.store(true, Ordering::SeqCst);
        let error = surface
            .call("TicketAssignmentAnswer", input.clone(), context("answer"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("outcome unknown"), "{error}");
        let writes = || {
            client
                .requests
                .lock()
                .unwrap()
                .iter()
                .filter(|(p, _, _)| p.ends_with("/assignment-answer"))
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(writes().len(), 1, "unknown outcome must not silently retry");
        let committed: serde_json::Value = serde_json::from_str(&writes()[0].2).unwrap();
        let receipt = surface
            .call("TicketAssignmentAnswer", input.clone(), context("answer"))
            .await
            .unwrap();
        assert_eq!(receipt, committed);
        assert_eq!(
            surface
                .call("TicketAssignmentAnswer", input.clone(), context("answer"))
                .await
                .unwrap(),
            receipt
        );
        assert_eq!(receipt["assignment_id"], observed[0]["assignment_id"]);
        let mut stale = input.clone();
        stale["operation_key"] = "stale-answer".into();
        stale["expected_last_event_id"] = "unobserved-event".into();
        let error = surface
            .call("TicketAssignmentAnswer", stale, context("stale"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("409"), "{error}");
        let mut refused = input.clone();
        refused["body"] = "Different answer under same operation".into();
        let error = surface
            .call("TicketAssignmentAnswer", refused, context("answer"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("409"), "{error}");
        // A valid Runtime signature is not a Worker membership grant. The
        // Backend must reject this unregistered source through the same tools.
        // Activate the real Feature while the source is registered, then lose
        // membership before dispatch. Otherwise activation itself correctly rejects
        // the fixture before it can exercise the operation's authentication check.
        let denied = ticket_client(&api, (*client.identity).clone(), &source);
        let denied_surface =
            TicketSurface::new(&dir.path().join("denied"), denied.clone(), wip).await;
        *denied.source.lock().unwrap() = RuntimeWorkerRef::new(
            source.runtime_id.clone(),
            "00000000-0000-7000-8000-000000000099",
        );
        let error = denied_surface
            .call("TicketAssignmentAnswer", input.clone(), context("denied"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("401"), "{error}");
        assert_eq!(
            runtime.input_requests.lock().unwrap().len(),
            input_count,
            "self Answer must not enqueue a new input to its own Orchestrator"
        );
        let saved = browser_ticket_backend(&api)
            .unwrap()
            .show(ticket.clone().into())
            .unwrap();
        let events: Vec<_> = saved
            .events
            .iter()
            .filter(|e| e.kind.as_str() == "assignment_answer")
            .collect();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].body.as_str(), input["body"].as_str().unwrap());
        assert_eq!(
            events[0].author.as_deref(),
            Some(format!("worker:{}/{}", source.runtime_id, source.worker_id).as_str())
        );
        assert_eq!(
            api.store
                .get_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket, &current.assignment_id)
                .unwrap()
                .unwrap(),
            current
        );
        assert_eq!(
            surface
                .call(
                    "TicketAssignmentTrackingQuery",
                    serde_json::json!({"ticket":ticket}),
                    context("read-after")
                )
                .await
                .unwrap(),
            observed
        );
    }
}

// Reviewer policy, tool projection, reference resolution, authentication and
// persistence remain production code. Only the synchronous HTTP transport is
// replaced by signed in-process router dispatch. Child spawning and MR attestation
// have their own suites; this fixture does not claim to exercise those boundaries.
fn reviewer_client(inner: Arc<TicketRouterClient>, bound_ticket: &str) -> Arc<dyn WorkspaceClient> {
    Arc::new(worker::ReviewerChildWorkspaceClient::new(
        inner,
        worker::ReviewerContext {
            ticket_id: bound_ticket.into(),
            merge_request_id: "unsubmitted-review-candidate".into(),
        },
        "unused-review-capability".into(),
    ))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reviewer_reads_saved_related_tickets_by_key_and_id_and_pages_their_threads() {
    for wip in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let (api, _runtime, identity, source, bound_id) = tracking_fixture(dir.path()).await;
        let backend = browser_ticket_backend(&api).unwrap();
        let mut related = ticket::NewTicket::new("Related requirements");
        related.body = ticket::MarkdownText::new("Requirements from a different Ticket");
        let related = backend.create(related).unwrap();
        backend
            .add_ticket_relation(
                bound_id.clone().into(),
                ticket::NewTicketRelation {
                    kind: ticket::TicketRelationKind::DependsOn,
                    target: related.id.clone(),
                    note: None,
                    author: None,
                },
            )
            .unwrap();
        for body in ["Earlier decision", "Latest decision"] {
            backend
                .add_event(
                    related.id.clone().into(),
                    ticket::NewTicketEvent::new(ticket::TicketEventKind::Comment, body),
                )
                .unwrap();
        }
        let bound = backend.show(bound_id.clone().into()).unwrap();
        let related_before = backend.show(related.id.clone().into()).unwrap();
        let bound_key = bound.meta.resource_key.as_deref().unwrap();
        let related_key = related.resource_key.as_deref().unwrap();
        let transport = ticket_client(&api, identity, &source);
        // One client must retain its review binding while following related Tickets.
        let client = reviewer_client(transport.clone(), &bound_id);
        let surface =
            TicketSurface::with_access(dir.path(), client, wip, TicketFeatureAccess::review())
                .await;
        for (reference, expected_key, expected_title, expected_body) in [
            (
                bound_key,
                bound_key,
                "Tracking authority",
                bound.document.body.as_str(),
            ),
            (
                related_key,
                related_key,
                "Related requirements",
                related_before.document.body.as_str(),
            ),
            (
                related.id.as_str(),
                related_key,
                "Related requirements",
                related_before.document.body.as_str(),
            ),
            (
                bound_id.as_str(),
                bound_key,
                "Tracking authority",
                bound.document.body.as_str(),
            ),
        ] {
            let shown = surface
                .call(
                    "ShowTicket",
                    serde_json::json!({"id":reference}),
                    ToolExecutionContext::direct(),
                )
                .await
                .unwrap();
            assert_eq!(shown["ticket"], expected_key);
            assert_eq!(shown["title"], expected_title);
            assert_eq!(shown["body"], expected_body);
            if expected_key == bound_key {
                assert!(
                    shown["relations"]["outgoing"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|relation| relation["ticket"] == related_key
                            && relation["kind"] == "depends_on")
                );
            }
        }
        let first = surface
            .call(
                "ShowTicket",
                serde_json::json!({"id":related_key,"event_limit":1}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        assert_eq!(first["thread"].as_array().unwrap().len(), 1);
        assert_eq!(first["thread"][0]["body"], "Latest decision");
        assert_eq!(first["event_page"]["has_more"], true);
        let second = surface.call("ShowTicket", serde_json::json!({"id":related_key,"event_limit":1,"event_cursor":first["event_page"]["next_cursor"]}), ToolExecutionContext::direct()).await.unwrap();
        assert_eq!(second["thread"].as_array().unwrap().len(), 1);
        assert_eq!(second["thread"][0]["body"], "Earlier decision");
        assert_eq!(backend.show(bound_id.into()).unwrap(), bound);
        assert_eq!(backend.show(related.id.into()).unwrap(), related_before);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reviewer_read_uses_live_backend_membership_and_cannot_mutate_saved_ticket() {
    for wip in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let (api, _runtime, identity, source, ticket) = tracking_fixture(dir.path()).await;
        let backend = browser_ticket_backend(&api).unwrap();
        let before = backend.show(ticket.clone().into()).unwrap();
        let transport = ticket_client(&api, identity, &source);
        let client = reviewer_client(transport.clone(), &ticket);
        let surface = TicketSurface::with_access(
            dir.path(),
            client.clone(),
            wip,
            TicketFeatureAccess::review(),
        )
        .await;
        surface
            .call(
                "ShowTicket",
                serde_json::json!({"id":ticket}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        let request_count = transport.requests.lock().unwrap().len();
        for (method, path, body) in [
            (
                WorkspaceRequestMethod::Post,
                format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}/comments"),
                serde_json::json!({"body":"unauthorized comment"}),
            ),
            (
                WorkspaceRequestMethod::Patch,
                format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{ticket}"),
                serde_json::json!({"title":"unauthorized title"}),
            ),
            (
                WorkspaceRequestMethod::Post,
                format!("/api/w/another-workspace/tickets/{ticket}/show"),
                serde_json::json!({}),
            ),
            (
                WorkspaceRequestMethod::Post,
                format!("/api/w/{TEST_WORKSPACE_ID}/merge-requests/other-candidate/reviews"),
                serde_json::json!({"verdict":"approved"}),
            ),
        ] {
            let client = client.clone();
            let error = tokio::task::spawn_blocking(move || {
                client.execute(WorkspaceRequest::json(method, path, body.to_string()))
            })
            .await
            .unwrap()
            .unwrap_err();
            assert!(error.to_string().contains("read-only"), "{error}");
        }
        assert_eq!(transport.requests.lock().unwrap().len(), request_count);
        assert_eq!(backend.show(ticket.clone().into()).unwrap(), before);
        // Keep a valid Runtime signature but select an unregistered Worker.
        // This must fail at the real Backend, not the Reviewer route filter.
        *transport.source.lock().unwrap() =
            RuntimeWorkerRef::new(source.runtime_id, uuid::Uuid::now_v7().to_string());
        let error = surface
            .call(
                "ShowTicket",
                serde_json::json!({"id":ticket}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("401"), "{error}");
        assert!(
            transport
                .requests
                .lock()
                .unwrap()
                .iter()
                .skip(request_count)
                .any(|(_, status, _)| *status == 401)
        );
        assert_eq!(backend.show(ticket.into()).unwrap(), before);
    }
}

fn report_counts(api: &WorkspaceApi) -> (i64, i64) {
    let conn = rusqlite::Connection::open(&api.config.database_path).unwrap();
    (
        conn.query_row(
            "SELECT count(*) FROM ticket_implementation_results",
            [],
            |r| r.get(0),
        )
        .unwrap(),
        conn.query_row(
            "SELECT count(*) FROM notification_sources WHERE kind='implementation_result'",
            [],
            |r| r.get(0),
        )
        .unwrap(),
    )
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn implementation_result_tool_and_wip_keep_host_owned_retry_identity_without_assignments() {
    for wip in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let (api, _runtime, identity, source, ticket) = tracking_fixture(dir.path()).await;
        // Deliberately do not call assign_tracking: reporting needs no delegation/MR.
        let before = browser_ticket_backend(&api)
            .unwrap()
            .show(ticket.clone().into())
            .unwrap();
        let client = ticket_client(&api, identity, &source);
        let surface = TicketSurface::new(dir.path(), client.clone(), wip).await;
        let input =
            serde_json::json!({"ticket":ticket,"kind":"result","body":"Repository-free findings"});
        client.lose_next_response.store(true, Ordering::SeqCst);
        let error = surface
            .call(
                "TicketImplementationResult",
                input.clone(),
                context("report-a"),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("outcome unknown"), "{error}");
        assert_eq!(report_counts(&api), (1, 1));
        let writes = || {
            client
                .requests
                .lock()
                .unwrap()
                .iter()
                .filter(|(p, _, _)| p.ends_with("/implementation-results"))
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(writes().len(), 1);
        let committed: serde_json::Value = serde_json::from_str(&writes()[0].2).unwrap();
        let receipt = surface
            .call(
                "TicketImplementationResult",
                input.clone(),
                context("report-a"),
            )
            .await
            .unwrap();
        assert_eq!(receipt, committed);
        assert_eq!(
            surface
                .call(
                    "TicketImplementationResult",
                    input.clone(),
                    context("report-a")
                )
                .await
                .unwrap(),
            receipt
        );
        assert_eq!(receipt["request"]["body"], input["body"]);
        assert_eq!(report_counts(&api), (1, 1));
        let queried = surface
            .call(
                "TicketImplementationResultQuery",
                serde_json::json!({"ticket":ticket,"result_id":receipt["result_id"]}),
                context("query"),
            )
            .await
            .unwrap();
        assert_eq!(queried, receipt);
        let mut changed = input.clone();
        changed["body"] = "Different body".into();
        let error = surface
            .call("TicketImplementationResult", changed, context("report-a"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("409"), "{error}");
        assert_eq!(report_counts(&api), (1, 1));
        let other = surface
            .call("TicketImplementationResult", input, context("report-b"))
            .await
            .unwrap();
        assert_ne!(other["result_id"], receipt["result_id"]);
        assert_eq!(report_counts(&api), (2, 2));
        assert_eq!(
            browser_ticket_backend(&api)
                .unwrap()
                .show(ticket.clone().into())
                .unwrap(),
            before
        );
        assert!(
            api.store
                .get_current_ticket_role_assignment(
                    TEST_WORKSPACE_ID,
                    &ticket,
                    TicketAssignmentRole::Orchestrator
                )
                .unwrap()
                .is_none()
        );
        assert!(
            api.store
                .get_active_ticket_role_assignment(
                    TEST_WORKSPACE_ID,
                    &ticket,
                    TicketAssignmentRole::Worker
                )
                .unwrap()
                .is_none()
        );
        let conn = rusqlite::Connection::open(&api.config.database_path).unwrap();
        let suppressed: i64 = conn.query_row("SELECT count(*) FROM notification_sources s JOIN notification_decisions d ON d.source_sequence=s.sequence WHERE s.kind='implementation_result' AND d.disposition='suppress' AND d.reason='no_recipient'", [], |r| r.get(0)).unwrap();
        assert_eq!(suppressed, 2);
    }
}
