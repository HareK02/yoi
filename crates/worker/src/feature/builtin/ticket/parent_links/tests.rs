//! Component boundary: production Tool/native conversion, runtime authorization and HTTP adapter;
//! the Workspace HTTP endpoint is a fixture, not evidence of server authorization or persistence.
use super::*;
use crate::worker::{WorkspaceClientError, WorkspaceResponse};
use assignment_tracking::authorization_tests::{AuthorizedClient, ticket};
use server_api::authorization::PermissionResource;

#[derive(Debug)]
struct Client {
    authority: Arc<AuthorizedClient>,
    requests: Mutex<Vec<WorkspaceRequest>>,
    response: Value,
    status: u16,
}
impl Client {
    fn new(response: Value) -> Arc<Self> {
        Arc::new(Self {
            authority: AuthorizedClient::new(json!(null)),
            requests: Mutex::new(Vec::new()),
            response,
            status: 200,
        })
    }
    fn grant(&self, operation: &str, id: &str) {
        self.authority.grant(operation, ticket(id));
    }
    fn writes(&self) -> Vec<WorkspaceRequest> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.method == WorkspaceRequestMethod::Put)
            .cloned()
            .collect()
    }
}
impl WorkspaceClient for Client {
    fn workspace_id(&self) -> Option<&str> {
        Some("workspace")
    }
    fn kind(&self) -> &str {
        "parent-link-boundary-fixture"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn execute(
        &self,
        request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        if request.path == "/api/w/workspace/authorization/check" {
            return self.authority.execute(request);
        }
        let path = request.path.clone();
        let resolving =
            request.method == WorkspaceRequestMethod::Get && path.ends_with("/link-summary");
        self.requests.lock().unwrap().push(request);
        let response = if resolving {
            let reference = path
                .strip_suffix("/link-summary")
                .unwrap()
                .rsplit('/')
                .next()
                .unwrap();
            let id = match reference {
                "T-1" => "internalchild",
                "T-2" => "internalold",
                "T-3" => "internalnew",
                _ => reference,
            };
            let key = match id {
                "internalchild" => "T-1",
                "internalold" => "T-2",
                "internalnew" => "T-3",
                _ => id,
            };
            summary(id, key)
        } else {
            self.response.clone()
        };
        Ok(WorkspaceResponse {
            status: if resolving { 200 } else { self.status },
            body: response.to_string(),
        })
    }
}

fn summary(id: &str, key: &str) -> Value {
    json!({
        "id": id,
        "resource_key": key,
        "title": "ordinary Ticket",
        "state": "closed",
        "closure_reason": "not_planned"
    })
}
fn link() -> Value {
    json!({
        "ticket_id": "internalchild",
        "observation": {"parent_ticket_id": "internalold", "last_operation_key": "before"},
        "parent": summary("internalold", "T-2")
    })
}
fn set_input(parent: Value) -> Value {
    json!({
        "ticket": "internalchild",
        "expected": {"parent_ticket_id": "internalold", "last_operation_key": "before"},
        "parent": parent,
        "operation_key": "same-key"
    })
}
fn arguments(value: Value) -> BTreeMap<String, WipValue> {
    match json_to_wip(&value).unwrap_or_else(|_| panic!("invalid WIP fixture arguments: {value}")) {
        WipValue::Record(fields) => fields,
        _ => panic!("expected record"),
    }
}
fn handler(client: Arc<Client>) -> TicketItemWipHandler {
    TicketItemWipHandler {
        tools: operation_map(
            native_ticket_tools(client, TicketFeatureAccess::workflow())
                .unwrap()
                .into_iter()
                .filter(|tool| tool.projection.surface == NativeTicketSurface::Item)
                .collect(),
        ),
        permissions: None,
        ticket_reference: "internalchild".into(),
        collection_route: "/cases".into(),
        merge_request_route: String::new(),
        objective_route: String::new(),
        observations: Arc::new(Mutex::new(TicketObservationState::default())),
    }
}
fn context() -> WipCallContext {
    WipCallContext {
        result_namespaces: Default::default(),
        execution: ToolExecutionContext::direct(),
        security_context: "worker".into(),
    }
}
fn native_set(parent: Value) -> BTreeMap<String, WipValue> {
    arguments(json!({
        "expected": {
            "parent_ticket_id": {"$case": "value", "value": "internalold"},
            "last_operation_key": {"$case": "value", "value": "before"}
        },
        "parent": parent,
        "operation_key": "same-key"
    }))
}

#[test]
fn set_schema_and_json_require_explicit_nullable_observations_and_desired_parent() {
    let schema = ParentToolKind::Set.schema();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let input = set_input(Value::Null);
    assert!(serde_json::from_value::<SetInput>(input.clone()).is_ok());
    assert!(validator.is_valid(&input));
    for path in [
        "parent",
        "expected.parent_ticket_id",
        "expected.last_operation_key",
    ] {
        let mut missing = input.clone();
        if let Some(field) = path.strip_prefix("expected.") {
            missing["expected"].as_object_mut().unwrap().remove(field);
        } else {
            missing.as_object_mut().unwrap().remove(path);
        }
        assert!(
            serde_json::from_value::<SetInput>(missing.clone()).is_err(),
            "{path}"
        );
        assert!(!validator.is_valid(&missing), "schema must require {path}");
    }
    let nulls = json!({
        "ticket": "T-1",
        "expected": {"parent_ticket_id": null, "last_operation_key": null},
        "parent": null,
        "operation_key": "k"
    });
    assert!(serde_json::from_value::<SetInput>(nulls.clone()).is_ok());
    assert!(validator.is_valid(&nulls));
    let linked = set_input(json!("T-3"));
    assert!(serde_json::from_value::<SetInput>(linked.clone()).is_ok());
    assert!(validator.is_valid(&linked));
    for parent in [json!(42), json!(false), json!({}), json!([])] {
        let malformed = set_input(parent);
        assert!(serde_json::from_value::<SetInput>(malformed.clone()).is_err());
        assert!(!validator.is_valid(&malformed));
    }
    assert!(
        schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("parent"))
    );
    let observation = &schema["$defs"]["TicketParentObservation"];
    for field in ["parent_ticket_id", "last_operation_key"] {
        assert!(
            observation["required"]
                .as_array()
                .unwrap()
                .contains(&json!(field))
        );
    }
}

#[tokio::test]
async fn reparent_requires_record_on_child_new_and_remove_on_child_old_in_tool_and_wip() {
    let client = Client::new(link());
    for operation in ["ticket.relation.record", "ticket.relation.remove"] {
        client.authority.grant(
            operation,
            PermissionResource::Workspace {
                workspace_id: "workspace".into(),
            },
        );
    }
    let (_, tool) = definition(client.clone(), ParentToolKind::Set)();
    let input = set_input(json!("T-3"));
    let wip = handler(client.clone());
    let args = native_set(json!({"$case":"value","value":"/cases/T-3"}));
    assert!(
        tool.execute(&input.to_string(), ToolExecutionContext::direct())
            .await
            .is_err()
    );
    assert!(wip.call("parent_set", &args, context()).await.is_err());
    assert!(client.requests.lock().unwrap().is_empty());

    for id in ["internalchild", "T-3", "internalnew"] {
        client.grant("ticket.relation.read", id);
    }
    for id in ["internalchild", "T-3", "internalnew"] {
        client.grant("ticket.relation.record", id);
    }
    // Even record authority on all three endpoints cannot detach the old edge.
    client.grant("ticket.relation.record", "internalold");
    assert!(
        tool.execute(&input.to_string(), ToolExecutionContext::direct())
            .await
            .is_err()
    );
    assert!(wip.call("parent_set", &args, context()).await.is_err());
    assert!(client.writes().is_empty());
    client.grant("ticket.relation.remove", "internalchild");
    assert!(
        tool.execute(&input.to_string(), ToolExecutionContext::direct())
            .await
            .is_err()
    );
    assert!(wip.call("parent_set", &args, context()).await.is_err());
    assert!(client.writes().is_empty());
    client.grant("ticket.relation.remove", "internalold");
    let result = tool
        .execute(&input.to_string(), ToolExecutionContext::direct())
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(result.content.as_deref().unwrap()).unwrap(),
        link()
    );
    wip.call("parent_set", &args, context())
        .await
        .unwrap_or_else(|_| panic!("WIP parent_set rejected authorized mutation"));
    let writes = client.writes();
    assert_eq!(writes.len(), 2);
    for write in writes {
        assert_eq!(write.path, "/api/w/workspace/tickets/internalchild/parent");
        let actual: Value = serde_json::from_str(write.body.as_deref().unwrap()).unwrap();
        assert_eq!(
            actual,
            json!({"expected":input["expected"],"parent":"T-3","operation_key":"same-key"})
        );
    }
    client.authority.grants.lock().unwrap().clear();
    assert!(wip.call("parent_set", &args, context()).await.is_err());
    assert_eq!(client.writes().len(), 2);
}

#[tokio::test]
async fn reparent_does_not_require_record_on_old_or_remove_on_new() {
    let client = Client::new(link());
    for id in ["internalchild", "T-3", "internalnew"] {
        client.grant("ticket.relation.read", id);
    }
    for id in ["internalchild", "T-3", "internalnew"] {
        client.grant("ticket.relation.record", id);
    }
    for id in ["internalchild", "internalold"] {
        client.grant("ticket.relation.remove", id);
    }
    let (_, tool) = definition(client.clone(), ParentToolKind::Set)();
    tool.execute(
        &set_input(json!("T-3")).to_string(),
        ToolExecutionContext::direct(),
    )
    .await
    .unwrap();
    handler(client.clone())
        .call(
            "parent_set",
            &native_set(json!({"$case":"value","value":"/cases/T-3"})),
            context(),
        )
        .await
        .unwrap_or_else(|_| panic!("WIP parent_set rejected authorized parent reference"));
    assert_eq!(client.writes().len(), 2);
    let checks = client.authority.authorizations.lock().unwrap();
    assert!(!checks.iter().any(|check| {
        check.operation == "ticket.relation.record" && check.resource == ticket("internalold")
    }));
    assert!(!checks.iter().any(|check| {
        check.operation == "ticket.relation.remove" && check.resource == ticket("internalnew")
    }));
    assert!(checks.iter().all(|check| check.operation != "ticket.read"));
    assert!(!checks.iter().any(|check| {
        check.operation == "ticket.relation.read" && check.resource == ticket("internalold")
    }));
    drop(checks);
    let requests = client.requests.lock().unwrap();
    let lookups = requests
        .iter()
        .filter(|request| request.method == WorkspaceRequestMethod::Get)
        .collect::<Vec<_>>();
    assert_eq!(lookups.len(), 2);
    assert!(
        lookups
            .iter()
            .all(|request| request.path == "/api/w/workspace/tickets/T-3/link-summary")
    );
}

#[tokio::test]
async fn same_parent_alias_is_compared_after_resolution_without_remove_or_wire_rewrite() {
    let client = Client::new(link());
    for id in ["internalchild", "T-2", "internalold"] {
        client.grant("ticket.relation.read", id);
        client.grant("ticket.relation.record", id);
    }
    let input = set_input(json!("T-2"));
    let (_, tool) = definition(client.clone(), ParentToolKind::Set)();
    tool.execute(&input.to_string(), ToolExecutionContext::direct())
        .await
        .unwrap();
    handler(client.clone())
        .call(
            "parent_set",
            &native_set(json!({"$case":"value","value":"/cases/T-2"})),
            context(),
        )
        .await
        .unwrap_or_else(|_| panic!("WIP parent_set rejected authorized parent reference"));
    assert_eq!(client.writes().len(), 2);
    assert!(
        client
            .authority
            .authorizations
            .lock()
            .unwrap()
            .iter()
            .all(|check| {
                check.operation != "ticket.relation.remove" && check.operation != "ticket.read"
            })
    );
    let requests = client.requests.lock().unwrap();
    let lookups = requests
        .iter()
        .filter(|request| request.method == WorkspaceRequestMethod::Get)
        .collect::<Vec<_>>();
    assert_eq!(lookups.len(), 2);
    assert!(
        lookups
            .iter()
            .all(|request| request.path == "/api/w/workspace/tickets/T-2/link-summary")
    );
    drop(requests);
    for write in client.writes() {
        let actual: Value = serde_json::from_str(write.body.as_deref().unwrap()).unwrap();
        assert_eq!(
            actual,
            json!({"expected":input["expected"],"parent":"T-2","operation_key":"same-key"})
        );
    }
}

#[tokio::test]
async fn initial_attach_needs_only_record_and_metadata_read_without_remove_or_body_lookup() {
    let client = Client::new(link());
    let (_, tool) = definition(client.clone(), ParentToolKind::Set)();
    let mut input = set_input(json!("T-3"));
    input["expected"] = json!({"parent_ticket_id":null,"last_operation_key":null});
    let args = arguments(json!({
        "expected": {
            "parent_ticket_id": {"$case": "null"},
            "last_operation_key": {"$case": "null"}
        },
        "parent": {"$case": "value", "value": "/cases/T-3"},
        "operation_key": "same-key"
    }));
    for id in ["internalchild", "T-3"] {
        client.grant("ticket.relation.record", id);
    }
    let wip = handler(client.clone());
    // Record grants never imply metadata read. Missing either endpoint rejects before PUT.
    assert!(
        tool.execute(&input.to_string(), ToolExecutionContext::direct())
            .await
            .is_err()
    );
    assert!(wip.call("parent_set", &args, context()).await.is_err());
    assert!(client.requests.lock().unwrap().is_empty());
    client.grant("ticket.relation.read", "internalchild");
    assert!(
        tool.execute(&input.to_string(), ToolExecutionContext::direct())
            .await
            .is_err()
    );
    assert!(wip.call("parent_set", &args, context()).await.is_err());
    assert!(client.requests.lock().unwrap().is_empty());
    client.grant("ticket.relation.read", "T-3");
    tool.execute(&input.to_string(), ToolExecutionContext::direct())
        .await
        .unwrap();
    wip.call("parent_set", &args, context())
        .await
        .unwrap_or_else(|_| panic!("WIP parent_set rejected authorized mutation"));
    assert_eq!(client.writes().len(), 2);
    assert!(
        client
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.method == WorkspaceRequestMethod::Put)
    );
    let checks = client.authority.authorizations.lock().unwrap();
    assert!(checks.iter().all(|check| matches!(
        check.operation.as_str(),
        "ticket.relation.record" | "ticket.relation.read"
    )));
    drop(checks);
    for write in client.writes() {
        let body: Value = serde_json::from_str(write.body.as_deref().unwrap()).unwrap();
        assert_eq!(
            body,
            json!({"expected":input["expected"],"parent":"T-3","operation_key":"same-key"})
        );
    }
}

#[tokio::test]
async fn unlink_needs_metadata_read_on_child_and_remove_on_child_old_only() {
    let client = Client::new(link());
    let (_, tool) = definition(client.clone(), ParentToolKind::Set)();
    let input = set_input(Value::Null);
    let args = native_set(json!({"$case":"null"}));
    let wip = handler(client.clone());
    for id in ["internalchild", "internalold"] {
        client.grant("ticket.relation.remove", id);
    }
    assert!(
        tool.execute(&input.to_string(), ToolExecutionContext::direct())
            .await
            .is_err()
    );
    assert!(wip.call("parent_set", &args, context()).await.is_err());
    assert!(client.requests.lock().unwrap().is_empty());
    client.grant("ticket.relation.read", "internalchild");
    tool.execute(&input.to_string(), ToolExecutionContext::direct())
        .await
        .unwrap();
    wip.call("parent_set", &args, context())
        .await
        .unwrap_or_else(|_| panic!("WIP parent_set rejected authorized mutation"));
    assert_eq!(client.writes().len(), 2);
    assert!(
        client
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.method == WorkspaceRequestMethod::Put)
    );
    let checks = client.authority.authorizations.lock().unwrap();
    assert!(checks.iter().all(|check| matches!(
        check.operation.as_str(),
        "ticket.relation.remove" | "ticket.relation.read"
    )));
    assert!(!checks.iter().any(|check| {
        check.operation == "ticket.relation.read" && check.resource == ticket("internalold")
    }));
}

#[tokio::test]
async fn children_transport_bounds_and_opaque_cursor_are_identical_on_tool_and_wip() {
    let page = json!({
        "parent_ticket_id": "internalchild",
        "items": [summary("internalold", "T-2")],
        "next_cursor": "next/+&=opaque",
        "has_more": true
    });
    let client = Client::new(page.clone());
    let (_, tool) = definition(client.clone(), ParentToolKind::Children)();
    let input = json!({"ticket":"internalchild","cursor":"page/+&=opaque","limit":1});
    assert!(
        tool.execute(&input.to_string(), ToolExecutionContext::direct())
            .await
            .is_err()
    );
    assert!(client.requests.lock().unwrap().is_empty());
    client.grant("ticket.relation.read", "internalchild");
    // Parent access alone cannot disclose an unauthorized child.
    assert!(
        tool.execute(&input.to_string(), ToolExecutionContext::direct())
            .await
            .is_err()
    );
    client.grant("ticket.relation.read", "internalold");
    let result = tool
        .execute(&input.to_string(), ToolExecutionContext::direct())
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(result.content.as_deref().unwrap()).unwrap(),
        page
    );
    let projected = results::project(
        "children_query",
        page,
        Some("internalchild"),
        "/cases",
        "",
        "",
    )
    .unwrap_or_else(|_| panic!("WIP children_query projection failed"));
    let WipValue::Record(fields) = projected else {
        panic!("children page")
    };
    assert_eq!(
        fields.get("entry"),
        Some(&WipValue::String("/cases/internalchild".into()))
    );
    assert_eq!(
        fields.get("next_cursor"),
        Some(&WipValue::String("next/+&=opaque".into()))
    );
    assert_eq!(fields.get("has_more"), Some(&WipValue::Boolean(true)));
    let Some(WipValue::List(items)) = fields.get("items") else {
        panic!("children")
    };
    let WipValue::Record(child) = &items[0] else {
        panic!("child")
    };
    assert_eq!(
        child.get("entry"),
        Some(&WipValue::String("/cases/T-2".into()))
    );
    let wip = handler(client.clone());
    wip.call(
        "children_query",
        &arguments(json!({"page":{"cursor":"page/+&=opaque","limit":1}})),
        context(),
    )
    .await
    .unwrap_or_else(|_| panic!("WIP children_query rejected authorized page request"));
    assert!(wip.observations.lock().unwrap().content_digests.is_empty());
    let requests = client.requests.lock().unwrap();
    assert!(requests.iter().all(|r| r.method == WorkspaceRequestMethod::Get
        && r.path == "/api/w/workspace/tickets/internalchild/children?limit=1&cursor=page%2F%2B%26%3Dopaque"));
    drop(requests);
    for limit in [0, 101] {
        let before = client.requests.lock().unwrap().len();
        assert!(
            tool.execute(
                &json!({"ticket":"internalchild","limit":limit}).to_string(),
                ToolExecutionContext::direct(),
            )
            .await
            .is_err()
        );
        assert_eq!(client.requests.lock().unwrap().len(), before);
    }
    let backend = WorkspaceHttpTicketBackend::new(client.clone());
    backend
        .query_ticket_children(
            TicketIdOrSlug::from("internalchild"),
            ticket::TicketChildrenRequest {
                cursor: None,
                limit: None,
            },
        )
        .unwrap();
    assert!(
        client
            .requests
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .path
            .ends_with("/children?limit=50")
    );
}

#[test]
fn children_read_checks_returned_parent_identity_not_only_requested_alias() {
    let client = Client::new(json!({
        "parent_ticket_id": "internalchild", "items": [], "next_cursor": null, "has_more": false
    }));
    client.grant("ticket.relation.read", "T-1");
    let backend = WorkspaceHttpTicketBackend::new(client.clone());
    let page = ticket::TicketChildrenRequest {
        cursor: None,
        limit: Some(1),
    };
    assert!(
        backend
            .query_ticket_children(TicketIdOrSlug::from("T-1"), page.clone())
            .is_err()
    );
    client.grant("ticket.relation.read", "internalchild");
    backend
        .query_ticket_children(TicketIdOrSlug::from("T-1"), page)
        .unwrap();
}

#[test]
fn children_adapter_rejects_oversized_server_page_without_silent_truncation() {
    let client = Client::new(json!({
        "parent_ticket_id": "T-1",
        "items": [summary("T-2", "T-2"), summary("T-3", "T-3")],
        "next_cursor": null,
        "has_more": false
    }));
    client.grant("ticket.relation.read", "T-1");
    let error = WorkspaceHttpTicketBackend::new(client)
        .query_ticket_children(
            TicketIdOrSlug::from("T-1"),
            ticket::TicketChildrenRequest {
                cursor: None,
                limit: Some(1),
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("bound"));
}

#[test]
fn native_parent_refs_are_scoped_and_observation_null_is_not_omission() {
    for parent in ["T-3", "/tickets/T-3", "/cases/T-3/children", "/cases/O-3"] {
        assert!(
            native::input(
                "parent_set",
                &native_set(json!({"$case":"value","value":parent})),
                Some("T-1"),
                Some("ticket"),
                "/cases",
                "",
                "",
            )
            .is_err()
        );
    }
    let args = native_set(json!({"$case":"null"}));
    let command = native::input(
        "parent_set",
        &args,
        Some("T-1"),
        Some("ticket"),
        "/cases",
        "",
        "",
    )
    .unwrap_or_else(|_| panic!("native parent_set rejected explicit null parent"));
    assert_eq!(command["parent"], Value::Null);
    for field in ["parent_ticket_id", "last_operation_key"] {
        let mut args = args.clone();
        if let Some(WipValue::Record(expected)) = args.get_mut("expected") {
            expected.remove(field);
        }
        assert!(
            native::input(
                "parent_set",
                &args,
                Some("T-1"),
                Some("ticket"),
                "/cases",
                "",
                "",
            )
            .is_err()
        );
    }
    let filtered = native::input(
        "query",
        &arguments(json!({"filter":{"parent":{"$case":"parent","value":{"ticket":"/cases/T-3"}}}})),
        None,
        None,
        "/cases",
        "",
        "",
    )
    .unwrap_or_else(|_| panic!("native query rejected scoped parent filter"));
    assert_eq!(filtered["parent"], json!({"kind":"parent","ticket":"T-3"}));
    let roots = native::input(
        "query",
        &arguments(json!({"filter":{"parent":{"$case":"none"}}})),
        None,
        None,
        "/cases",
        "",
        "",
    )
    .unwrap_or_else(|_| panic!("native query rejected root-only parent filter"));
    assert_eq!(roots["parent"], json!({"kind":"none"}));
}

#[test]
fn parent_read_preserves_cas_ids_and_requires_actual_parent_resource() {
    let client = Client::new(link());
    client.grant("ticket.relation.read", "internalchild");
    let backend = WorkspaceHttpTicketBackend::new(client.clone());
    assert!(
        backend
            .get_ticket_parent(TicketIdOrSlug::from("internalchild"))
            .is_err()
    );
    client.grant("ticket.relation.read", "internalold");
    let result = backend
        .get_ticket_parent(TicketIdOrSlug::from("internalchild"))
        .unwrap();
    assert_eq!(
        result.observation.parent_ticket_id.as_deref(),
        Some("internalold")
    );
    assert_eq!(result.parent.unwrap().id, "internalold");
    assert!(
        client
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|r| r.path.ends_with("/internalchild/parent"))
    );
}

#[tokio::test]
async fn show_and_wip_read_expose_parent_observation_and_lazy_navigation() {
    let mut detail = super::super::tests::target_detail_fixture(json!([]));
    detail["resource_key"] = json!("T-1");
    detail["parent_link"] = link();
    let client = Client::new(detail);
    client.grant("ticket.read", "internalchild");
    client.grant("ticket.relation.read", "internalchild");
    let (_, tool) =
        workspace_ticket_read_definition(client.clone(), WorkspaceTicketReadKind::Show)();
    assert!(
        tool.execute(r#"{"id":"internalchild"}"#, ToolExecutionContext::direct())
            .await
            .is_err()
    );
    client.grant("ticket.relation.read", "internalold");
    let output = tool
        .execute(r#"{"id":"internalchild"}"#, ToolExecutionContext::direct())
        .await
        .unwrap();
    let value: Value = serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
    assert_eq!(value["parent_link"], link());
    assert_eq!(
        value["parent_link"]["parent"]["closure_reason"],
        "not_planned"
    );
    handler(client.clone())
        .call("read", &BTreeMap::new(), context())
        .await
        .unwrap_or_else(|_| panic!("WIP read rejected authorized parent metadata"));
    let projected = results::project("read", value, Some("internalchild"), "/cases", "", "")
        .unwrap_or_else(|_| panic!("WIP read projection failed for parent metadata"));
    let WipValue::Record(fields) = projected else {
        panic!("envelope")
    };
    let Some(WipValue::Record(links)) = fields.get("links") else {
        panic!("links")
    };
    let Some(WipValue::List(tickets)) = links.get("tickets") else {
        panic!("tickets")
    };
    assert!(tickets.iter().any(|item| {
        matches!(
            item,
            WipValue::Record(fields)
                if fields.get("entry") == Some(&WipValue::String("/cases/T-2".into()))
        )
    }));
    // Navigation does not eagerly read the parent.
    assert!(
        client
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|r| r.path.ends_with("/internalchild/show"))
    );
}

#[tokio::test]
async fn show_rejects_missing_null_or_malformed_parent_link_without_inventing_observation() {
    for parent_link in [
        None,
        Some(Value::Null),
        Some(json!({
            "ticket_id": "internalchild",
            "observation": {"parent_ticket_id": null},
            "parent": null
        })),
    ] {
        let mut detail = super::super::tests::target_detail_fixture(json!([]));
        if let Some(parent_link) = parent_link {
            detail["parent_link"] = parent_link;
        } else {
            detail.as_object_mut().unwrap().remove("parent_link");
        }
        let client = Client::new(detail);
        client.grant("ticket.read", "internalchild");
        client.grant("ticket.relation.read", "internalchild");
        let (_, tool) =
            workspace_ticket_read_definition(client.clone(), WorkspaceTicketReadKind::Show)();
        let error = tool
            .execute(r#"{"id":"internalchild"}"#, ToolExecutionContext::direct())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("parent_link"));
        assert!(
            handler(client)
                .call("read", &BTreeMap::new(), context())
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn parent_query_filter_is_forwarded_unchanged_and_requires_target_not_collection_only() {
    for parent in [
        json!({"kind":"none"}),
        json!({"kind":"parent","ticket":"T-3"}),
    ] {
        let client = Client::new(json!({
            "items": [], "page": {"next_cursor": null, "has_more": false}
        }));
        client.authority.grant(
            "ticket.query",
            PermissionResource::Workspace {
                workspace_id: "workspace".into(),
            },
        );
        let (_, tool) =
            workspace_ticket_read_definition(client.clone(), WorkspaceTicketReadKind::Query)();
        let input = json!({"parent":parent,"cursor":"opaque/+&","limit":7});
        if parent["kind"] == "parent" {
            assert!(
                tool.execute(&input.to_string(), ToolExecutionContext::direct())
                    .await
                    .is_err()
            );
            assert!(client.requests.lock().unwrap().is_empty());
            client.grant("ticket.relation.query", "T-3");
        }
        tool.execute(&input.to_string(), ToolExecutionContext::direct())
            .await
            .unwrap();
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/api/w/workspace/tickets/query");
        let body: Value = serde_json::from_str(requests[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(body["parent"], parent);
        assert_eq!(body["cursor"], "opaque/+&");
        assert_eq!(body["limit"], 7);
    }
}

#[tokio::test]
async fn failed_parent_write_never_refreshes_cas_or_retries_as_a_new_operation() {
    for status in [409, 503] {
        let client = Arc::new(Client {
            authority: AuthorizedClient::new(json!(null)),
            requests: Mutex::new(Vec::new()),
            response: json!({"message":"reconcile current parent"}),
            status,
        });
        for id in ["internalchild", "internalold"] {
            client.grant("ticket.relation.remove", id);
        }
        client.grant("ticket.relation.read", "internalchild");
        let (_, tool) = definition(client.clone(), ParentToolKind::Set)();
        let input = set_input(Value::Null);
        for attempt in 1..=2 {
            assert!(
                tool.execute(&input.to_string(), ToolExecutionContext::direct())
                    .await
                    .is_err()
            );
            assert_eq!(client.writes().len(), attempt); // no hidden mutation retry
        }
        let writes = client.writes();
        assert_eq!(writes[0], writes[1]);
        let body: Value = serde_json::from_str(writes[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(body["expected"], input["expected"]);
        assert_eq!(body["operation_key"], "same-key");
    }
}
