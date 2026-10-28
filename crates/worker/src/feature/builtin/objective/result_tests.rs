//! Model-facing integration of production Inspect/Text View, Invoke, native Objective projection,
//! Client result validation and selected Ticket navigation. Only Workspace HTTP/authorization replies
//! are scripted (no server policy or persistence claim). Normal crate tests include this suite.

use super::tests::{MockWorkspaceClient, objective_response};
use super::*;
use crate::wip::{WipRuntime, install_wip_mode_with_mounts};
use agen::Engine;
use agen::llm_client::client::LlmClient;
use agen::llm_client::{ClientError, Request as LlmRequest, ResponseStream};
use agen::state::Mutable;
use std::sync::atomic::Ordering;

#[derive(Clone)]
struct NoLlm;
#[async_trait]
impl LlmClient for NoLlm {
    async fn stream(&self, _: LlmRequest) -> Result<ResponseStream, ClientError> {
        panic!("model-facing tools are driven explicitly, not by an LLM fixture")
    }
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
}

struct Model {
    engine: Engine<NoLlm, Mutable, ()>,
    runtime: Arc<WipRuntime>,
}

impl Model {
    // allocated and mounted are deliberately independent; Objective never enables Ticket itself.
    fn new(
        client: Arc<MockWorkspaceClient>,
        allocated: bool,
        mounted: bool,
        objective_permissions: Option<ToolPermissionConfig>,
    ) -> Self {
        let mut registry = WipMountRegistry::new();
        let ticket = allocated.then(|| registry.allocate_namespace("ticket", "cases").unwrap());
        let objective = registry.allocate_namespace("objective", "goals").unwrap();
        mount_workspace_http_objective_wip(
            &mut registry,
            client.clone(),
            objective_permissions,
            &objective,
        )
        .unwrap();
        // Mount after Objective: final Host publication, not mount-time allocation, supplies links.
        if mounted {
            super::super::ticket::mount_workspace_http_ticket_wip(
                &mut registry,
                client,
                super::super::ticket::TicketFeatureAccess::workspace_authoring(),
                None,
                ticket.as_ref().unwrap(),
            )
            .unwrap();
        }
        let mut engine = Engine::new_annotated(NoLlm);
        let runtime =
            install_wip_mode_with_mounts(&mut engine, None, "objective-model".into(), registry)
                .unwrap();
        engine.tool_server_handle().flush_pending();
        Self { engine, runtime }
    }

    async fn execute(&self, name: &str, input: Json) -> Result<Json, ToolError> {
        let output = self
            .engine
            .tool_server_handle()
            .call_tool(name, &input.to_string(), ToolExecutionContext::direct())
            .await
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        // Parse only the actual model ToolOutput envelope, never Text View or arbitrary domain JSON.
        Ok(serde_json::from_str(output.content.as_deref().unwrap()).unwrap())
    }

    async fn inspect(&self, entry: &Json) -> Result<Json, ToolError> {
        self.execute("Inspect", json!({"path":entry})).await
    }

    async fn contract(&self, object: &Json) -> Json {
        self.inspect(&object["interfaces"][0]["path"])
            .await
            .unwrap()
    }

    async fn invoke(
        &self,
        object: &Json,
        operation: &str,
        arguments: Json,
    ) -> Result<Json, ToolError> {
        self.execute(
            "Invoke",
            json!({
                "path":object["path"], "interface":object["interfaces"][0]["reference"],
                "operation":operation, "arguments":arguments,
            }),
        )
        .await
    }
}

fn response(body: Json) -> crate::worker::WorkspaceResponse {
    crate::worker::WorkspaceResponse {
        status: 200,
        body: body.to_string(),
    }
}

fn query_item(id: &str, tickets: Json) -> Json {
    json!({"resource_key":id,"title":"Intent","snippet":"O-999 /cases/T-999 are text only",
        "state":"active","linked_ticket_keys":tickets})
}

fn query(items: Json, cursor: Json, more: bool) -> crate::worker::WorkspaceResponse {
    response(json!({"items":items,"page":{"next_cursor":cursor,"has_more":more}}))
}

fn detail(tickets: Json) -> crate::worker::WorkspaceResponse {
    let mut body: Json = serde_json::from_str(&objective_response("Intent").body).unwrap();
    body["linked_ticket_summaries"] = tickets;
    body["body"] = json!("T-999 /cases/T-999 {\"objective\":\"O-999\"} remain content");
    body["events"] = json!([{"kind":"edited","created_at":"now","body":"T-888"}]);
    body["event_page"] = json!({"next_cursor":"events-next","has_more":true});
    response(body)
}

fn linked_ticket() -> Json {
    json!([{"resource_key":"T-7","title":"Linked","state":"open"}])
}

fn ticket_detail() -> crate::worker::WorkspaceResponse {
    response(json!({
        "resource_key":"T-7","title":"Linked","body":"Requirements","body_truncated":false,
        "content_digest":"observed","state":"open","targets":[],"events":[],"linked_objectives":[],
        "initial_post":{"sequence":0,"event_ref":"initial","is_initial":true,"kind":"comment",
            "body":"Requirements","body_truncated":false,"body_digest":"a".repeat(64),
            "editable":true,"author":null,"at":null,"edited_at":null,"edited_by":null},
        "closure_operation_key":null,"current_closure":null,"legacy_handoff":null,"closure_history":[],
        "assignment_tracking":[],"tags":[],"assignments":[],"implementation_reports":[],"merge_requests":[]
    }))
}

fn error_response(status: u16) -> crate::worker::WorkspaceResponse {
    crate::worker::WorkspaceResponse {
        status,
        body: "unavailable".into(),
    }
}

fn validate(descriptor: &InterfaceDescriptor, operation: &str, output: &Json) {
    descriptor
        .validate_result(operation, &json_to_wip(output).unwrap())
        .unwrap();
}

#[tokio::test]
async fn model_query_selected_entry_read_paging_and_empty_are_lazy_nonindexable() {
    let client = Arc::new(MockWorkspaceClient::new([
        query(
            json!([
                query_item("O-3", json!(["T-7"])),
                query_item("O-4", json!([]))
            ]),
            json!("next-page"),
            true,
        ),
        detail(linked_ticket()),
        query(json!([]), Json::Null, false),
    ]));
    let model = Model::new(client.clone(), true, true, None);
    let collection = model.inspect(&json!("/goals")).await.unwrap();
    let contract = model.contract(&collection).await;
    let signature = contract["interface_signature"].as_str().unwrap();
    assert!(signature.contains("data: json"), "{signature}");
    assert!(signature.contains("`entry`: entry"), "{signature}");
    assert!(signature.contains("`entry`?: entry"), "{signature}");
    assert!(signature.contains("next_cursor?: string"), "{signature}");
    let before = model.runtime.metrics();
    let found = model
        .invoke(&collection, "query", json!({"limit":2}))
        .await
        .unwrap();
    validate(&objective_collection_descriptor(), "query", &found);
    assert_eq!(found["next_cursor"], "next-page");
    assert_eq!(found["has_more"], true);
    assert_eq!(found["items"].as_array().unwrap().len(), 2);
    assert!(
        found.get("data").is_none(),
        "query items must not be duplicated"
    );
    assert_eq!(found["items"][0]["data"]["linked_tickets"], json!(["T-7"]));
    assert_eq!(
        found["items"][0]["links"]["tickets"],
        json!([{"id":"T-7","entry":"/cases/T-7"}])
    );
    assert_eq!(found["items"][1]["links"]["tickets"], json!([]));
    assert_eq!(client.requests().len(), 1, "no eager item or Ticket reads");
    assert_eq!(
        model.runtime.metrics().inspect_round_trips,
        before.inspect_round_trips
    );
    assert_eq!(
        model.runtime.metrics().discover_round_trips,
        before.discover_round_trips
    );
    // Select the declared Entry field from the Invoke envelope, not a path inferred from data.
    let selected = model.inspect(&found["items"][0]["entry"]).await.unwrap();
    let item_contract = model.contract(&selected).await;
    assert!(
        item_contract["interface_signature"]
            .as_str()
            .unwrap()
            .contains("`entry`: entry")
    );
    assert_eq!(
        client.requests().len(),
        1,
        "fixed signatures do not read content"
    );
    let read = model
        .invoke(
            &selected,
            "read",
            json!({"event_limit":1,"event_cursor":"events-page"}),
        )
        .await
        .unwrap();
    validate(&objective_item_descriptor(), "read", &read);
    assert_eq!(read["entry"], found["items"][0]["entry"]);
    assert_eq!(
        read["data"]["event_page"],
        json!({"next_cursor":"events-next","has_more":true})
    );
    assert_eq!(read["data"]["events"][0]["body"], "T-888");
    assert_eq!(
        read["links"]["tickets"].as_array().unwrap().len(),
        1,
        "body/events/snippets cannot become links"
    );
    let empty = model
        .invoke(
            &collection,
            "query",
            json!({"limit":2,"cursor":found["next_cursor"]}),
        )
        .await
        .unwrap();
    validate(&objective_collection_descriptor(), "query", &empty);
    assert_eq!(empty, json!({"items":[],"has_more":false}));
    let tree = model
        .execute(
            "Tree",
            json!({"path":collection["path"],"depth":2,"refresh":true}),
        )
        .await
        .unwrap();
    assert_eq!(
        tree["tree"]["children"],
        json!([]),
        "selected items never become indexable edges"
    );
    let requests = client.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[1].path.ends_with("/objectives/O-3/show"));
    assert_eq!(
        serde_json::from_str::<Json>(requests[1].body.as_deref().unwrap()).unwrap(),
        json!({"event_limit":1,"event_cursor":"events-page"})
    );
    assert_eq!(
        serde_json::from_str::<Json>(requests[2].body.as_deref().unwrap()).unwrap()["cursor"],
        found["next_cursor"]
    );
    assert!(!requests.iter().any(|r| r.path.contains("O-4")));
}

#[tokio::test]
async fn model_query_omits_ticket_entries_for_unallocated_or_unpublished_namespaces() {
    for allocated in [false, true] {
        let client = Arc::new(MockWorkspaceClient::new([query(
            json!([query_item("O-3", json!(["T-7"]))]),
            Json::Null,
            false,
        )]));
        let model = Model::new(client.clone(), allocated, false, None);
        let collection = model.inspect(&json!("/goals")).await.unwrap();
        model.contract(&collection).await;
        let found = model.invoke(&collection, "query", json!({})).await.unwrap();
        validate(&objective_collection_descriptor(), "query", &found);
        assert_eq!(found["items"][0]["links"]["tickets"], json!([{"id":"T-7"}]));
        assert_eq!(found["items"][0]["data"]["linked_tickets"], json!(["T-7"]));
        assert_eq!(found["items"][0]["entry"], "/goals/O-3");
        assert_eq!(client.requests().len(), 1);
    }
}

#[tokio::test]
async fn model_create_subject_and_related_ticket_entries_are_inspected_then_read() {
    let client = Arc::new(MockWorkspaceClient::new([
        detail(linked_ticket()),
        detail(linked_ticket()),
        ticket_detail(),
    ]));
    let model = Model::new(client.clone(), true, true, None);
    let collection = model.inspect(&json!("/goals")).await.unwrap();
    model.contract(&collection).await;
    let created = model
        .invoke(
            &collection,
            "create",
            json!({"title":"Intent","linked_tickets":["T-7"]}),
        )
        .await
        .unwrap();
    validate(&objective_collection_descriptor(), "create", &created);
    assert_eq!(
        client.requests().len(),
        1,
        "creating a result does not fetch any references"
    );
    let selected = model.inspect(&created["entry"]).await.unwrap();
    model.contract(&selected).await;
    let read = model.invoke(&selected, "read", json!({})).await.unwrap();
    assert_eq!(read["data"]["objective"], created["data"]["objective"]);
    let ticket = model
        .inspect(&created["links"]["tickets"][0]["entry"])
        .await
        .unwrap();
    let ticket_contract = model.contract(&ticket).await;
    assert!(
        ticket_contract["interface_signature"]
            .as_str()
            .unwrap()
            .contains("read(")
    );
    let linked = model.invoke(&ticket, "read", json!({})).await.unwrap();
    assert_eq!(linked["data"]["ticket"], "T-7"); // Existing Ticket Model data, not an inferred coordinate.
    let requests = client.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.path == "/api/w/workspace/objectives")
            .count(),
        1
    );
    assert!(requests[2].path.ends_with("/tickets/T-7/show"));
}

#[tokio::test]
async fn model_allocation_is_not_publication_and_followup_failure_never_replays_create() {
    let client = Arc::new(MockWorkspaceClient::new([
        detail(linked_ticket()),
        error_response(404),
    ]));
    let model = Model::new(client.clone(), true, false, None);
    let collection = model.inspect(&json!("/goals")).await.unwrap();
    model.contract(&collection).await;
    let created = model
        .invoke(&collection, "create", json!({"title":"Intent"}))
        .await
        .unwrap();
    assert_eq!(created["links"]["tickets"], json!([{"id":"T-7"}]));
    assert!(
        created["links"]["tickets"][0].get("entry").is_none(),
        "allocated but unpublished spaces must not supply navigable entries"
    );
    assert_eq!(
        client.requests().len(),
        1,
        "building references does not fetch Tickets"
    );
    let selected = model.inspect(&created["entry"]).await.unwrap();
    model.contract(&selected).await;
    let deleted = model
        .invoke(&selected, "read", json!({}))
        .await
        .unwrap_err();
    assert!(deleted.to_string().contains("NotFound"), "{deleted}");
    assert_eq!(model.runtime.metrics().operation_successes, 1);
    assert_eq!(client.requests().len(), 2);
    assert_eq!(
        client
            .requests()
            .iter()
            .filter(|r| r.path == "/api/w/workspace/objectives")
            .count(),
        1
    );
    validate(&objective_collection_descriptor(), "create", &created);
}

#[tokio::test]
async fn model_unallocated_ticket_retains_id_and_backend_read_denial_is_not_empty_success() {
    let client = Arc::new(MockWorkspaceClient::new([
        detail(linked_ticket()),
        error_response(403),
    ]));
    let model = Model::new(client.clone(), false, false, None);
    let collection = model.inspect(&json!("/goals")).await.unwrap();
    let created = model
        .invoke(&collection, "create", json!({"title":"Intent"}))
        .await
        .unwrap();
    assert_eq!(created["links"]["tickets"], json!([{"id":"T-7"}]));
    validate(&objective_collection_descriptor(), "create", &created);
    let selected = model.inspect(&created["entry"]).await.unwrap();
    let denied = model
        .invoke(&selected, "read", json!({}))
        .await
        .unwrap_err();
    assert!(denied.to_string().contains("PermissionDenied"), "{denied}");
    assert_eq!(client.requests().len(), 2);
    assert_eq!(model.runtime.metrics().operation_successes, 1);
}

#[tokio::test]
async fn model_selected_related_ticket_denied_or_deleted_does_not_repeat_objective_mutation() {
    for status in [403, 404] {
        let client = Arc::new(MockWorkspaceClient::new([
            detail(linked_ticket()),
            error_response(status),
        ]));
        let model = Model::new(client.clone(), true, true, None);
        let collection = model.inspect(&json!("/goals")).await.unwrap();
        let created = model
            .invoke(&collection, "create", json!({"title":"Intent"}))
            .await
            .unwrap();
        let related = model
            .inspect(&created["links"]["tickets"][0]["entry"])
            .await
            .unwrap();
        model.contract(&related).await;
        let error = model.invoke(&related, "read", json!({})).await.unwrap_err();
        assert!(
            error.to_string().contains(if status == 403 {
                "PermissionDenied"
            } else {
                "NotFound"
            }),
            "{error}"
        );
        assert_eq!(client.requests().len(), 2);
        assert_eq!(model.runtime.metrics().operation_successes, 1);
        validate(&objective_collection_descriptor(), "create", &created);
    }
}

#[tokio::test]
async fn model_permissions_remain_call_time_and_pre_dispatch() {
    let client = Arc::new(MockWorkspaceClient::new([detail(linked_ticket())]));
    let model = Model::new(client.clone(), true, false, None);
    let collection = model.inspect(&json!("/goals")).await.unwrap();
    let created = model
        .invoke(&collection, "create", json!({"title":"Intent"}))
        .await
        .unwrap();
    client.auth_denied.store(true, Ordering::SeqCst);
    let selected = model.inspect(&created["entry"]).await.unwrap();
    let error = model
        .invoke(&selected, "read", json!({}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("PermissionDenied"), "{error}");
    assert_eq!(client.requests().len(), 1);
    let checked = client.authorizations.lock().unwrap();
    assert_eq!(checked.last().unwrap().operation, "objective.read");
    drop(checked);
    let denied_model = Model::new(
        client.clone(),
        true,
        false,
        Some(ToolPermissionConfig {
            default_action: ToolPermissionAction::Deny,
            rules: vec![],
        }),
    );
    let target = denied_model.inspect(&created["entry"]).await.unwrap();
    let error = denied_model
        .invoke(&target, "read", json!({}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("PermissionDenied"), "{error}");
    assert_eq!(client.requests().len(), 1);
}

#[tokio::test]
async fn model_item_mutations_share_descriptor_value_parity_and_preserve_domain_content() {
    let lookup = || response(json!({"resource_key":"T-7"}));
    let client = Arc::new(MockWorkspaceClient::new([
        detail(linked_ticket()),
        detail(linked_ticket()),
        lookup(),
        detail(linked_ticket()),
        lookup(),
        detail(json!([])),
    ]));
    let model = Model::new(client.clone(), true, false, None);
    let selected = model.inspect(&json!("/goals/O-3")).await.unwrap();
    model.contract(&selected).await;
    for (operation, arguments) in [
        ("edit", json!({"title":"Edited"})),
        ("set_state", json!({"state":"active"})),
        ("link_ticket", json!({"ticket_id":"T-7"})),
        ("unlink_ticket", json!({"ticket_id":"T-7"})),
    ] {
        let result = model.invoke(&selected, operation, arguments).await.unwrap();
        validate(&objective_item_descriptor(), operation, &result);
        assert_eq!(result["entry"], "/goals/O-3");
        assert!(result["data"]["body"].as_str().unwrap().contains("T-999"));
        assert_eq!(result["data"]["event_page"]["next_cursor"], "events-next");
        assert_eq!(
            result["links"]["tickets"].as_array().unwrap().len(),
            if operation == "unlink_ticket" { 0 } else { 1 }
        );
    }
    assert_eq!(
        client.requests().len(),
        6,
        "only existing link/unlink preflight reads, no result reference fetches"
    );
}

#[test]
fn result_contract_rejects_wrong_entry_types_and_does_not_guess_links_from_json() {
    let projected = json!({"objective":"O-3","linked_tickets":[{"ticket":"T-7"},{"ticket":"../hidden"}],
        "body":"T-90 /cases/T-90", "events":[{"ticket":"T-91"}],"merge_request_id":"MR-92"});
    let routes = results::Routes {
        objective: "/goals".into(),
    };
    let value = routes
        .detail(&projected, Some("/cases"))
        .unwrap_or_else(|_| panic!("valid projected detail"));
    let descriptor = objective_item_descriptor();
    descriptor.validate_result("read", &value).unwrap();
    let output = wip_to_json(&value).unwrap();
    assert_eq!(
        output["links"]["tickets"],
        json!([{"id":"T-7","entry":"/cases/T-7"},{"id":"../hidden"}])
    );
    assert_eq!(output["data"], projected);
    let mut invalid = output.clone();
    invalid["entry"] = json!(7);
    assert!(
        descriptor
            .validate_result("read", &json_to_wip(&invalid).unwrap())
            .is_err()
    );
    invalid = output;
    invalid["links"]["tickets"][0]["entry"] = json!("not-an-absolute-coordinate");
    assert!(
        descriptor
            .validate_result("read", &json_to_wip(&invalid).unwrap())
            .is_err()
    );
    let unavailable = routes
        .detail(&projected, None)
        .unwrap_or_else(|_| panic!("valid projected detail without published Ticket namespace"));
    assert_eq!(
        wip_to_json(&unavailable).unwrap()["links"]["tickets"],
        json!([{"id":"T-7"},{"id":"../hidden"}])
    );
}
