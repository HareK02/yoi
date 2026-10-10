use super::*;
use crate::worker::{WorkspaceClientError, WorkspaceResponse};

#[derive(Debug)]
struct RecordingClient {
    requests: Mutex<Vec<WorkspaceRequest>>,
    body: String,
    identity: bool,
    status: u16,
}
impl WorkspaceClient for RecordingClient {
    fn workspace_id(&self) -> Option<&str> {
        self.identity.then_some("workspace")
    }
    fn kind(&self) -> &str {
        "classification-test"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn execute(
        &self,
        request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        self.requests.lock().unwrap().push(request);
        Ok(WorkspaceResponse {
            status: self.status,
            body: self.body.clone(),
        })
    }
}
fn recording(body: Value) -> Arc<RecordingClient> {
    Arc::new(RecordingClient {
        requests: Mutex::new(Vec::new()),
        body: body.to_string(),
        identity: true,
        status: 200,
    })
}
fn tag() -> Value {
    json!({"workspace_id":"workspace", "id":"tag-1", "name":"Ready", "last_operation_key":"create-1", "deleted":false})
}
fn cases() -> Vec<(TagToolKind, Value, &'static str)> {
    vec![
        (TagToolKind::List, json!({"limit":5}), "WorkspaceTagList"),
        (
            TagToolKind::Create,
            json!({"name":"Ready", "operation_key":"create-1"}),
            "WorkspaceTagCreate",
        ),
        (
            TagToolKind::Rename,
            json!({"tag_id":"tag-1", "expected_last_operation_key":"create-1", "name":"New", "operation_key":"rename-1"}),
            "WorkspaceTagRename",
        ),
        (
            TagToolKind::Delete,
            json!({"tag_id":"tag-1", "expected_last_operation_key":"create-1", "operation_key":"delete-1"}),
            "WorkspaceTagDelete",
        ),
        (
            TagToolKind::Set,
            json!({"ticket":"T-740", "expected_tag_ids":[], "tag_ids":["tag-1", "tag-2"], "operation_key":"set-1"}),
            "TicketTagsSet",
        ),
    ]
}

#[tokio::test]
async fn classification_tool_schemas_require_observations_and_reject_actor_claims() {
    for (kind, input, name) in cases() {
        let client = recording(json!([]));
        let (meta, tool) = definition(client.clone(), kind)();
        assert_eq!(meta.name, name);
        let validator = jsonschema::validator_for(&meta.input_schema).unwrap();
        assert!(validator.is_valid(&input), "{name}: {}", meta.input_schema);
        for field in meta.input_schema["required"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let field = field.as_str().unwrap();
            let mut invalid = input.clone();
            invalid.as_object_mut().unwrap().remove(field);
            assert!(!validator.is_valid(&invalid), "{name}.{field}");
            assert!(matches!(
                tool.execute(&invalid.to_string(), ToolExecutionContext::direct())
                    .await,
                Err(ToolError::InvalidArgument(_))
            ));
        }
        let mut invalid = input.clone();
        invalid["author"] = json!("orchestrator");
        assert!(!validator.is_valid(&invalid));
        assert!(matches!(
            tool.execute(&invalid.to_string(), ToolExecutionContext::direct())
                .await,
            Err(ToolError::InvalidArgument(_))
        ));
        assert!(client.requests.lock().unwrap().is_empty());
    }
    let schema = TagToolKind::Set.schema();
    assert_eq!(schema["properties"]["tag_ids"]["maxItems"], 100);
    assert_eq!(schema["properties"]["expected_tag_ids"]["maxItems"], 100);
    assert_eq!(
        TagToolKind::List.schema()["properties"]["limit"]["maximum"],
        200
    );
}

#[tokio::test]
async fn classification_reads_forward_id_filters_and_preserve_tag_observations() {
    let item = json!({"resource_key":"T-740", "title":"Classify", "state":"planning", "tags":[tag()], "matched_fields":[], "linked_objective_keys":[], "relation_count":0, "blocker_count":0, "unresolved_blocker_count":0, "unresolved_review_count":0, "merge_requests":[]});
    let client = recording(json!({"items":[item], "page":{"has_more":false, "next_cursor":null}}));
    let (meta, tool) =
        workspace_ticket_read_definition(client.clone(), WorkspaceTicketReadKind::Query)();
    for field in ["tag_ids", "tag_ids_any", "tag_ids_none"] {
        assert_eq!(meta.input_schema["properties"][field]["maxItems"], 100);
    }
    let input = json!({"tag_ids":["tag-1"], "tag_ids_any":["tag-2", "tag-3"], "tag_ids_none":["tag-4"], "limit":7});
    let output = tool
        .execute(&input.to_string(), ToolExecutionContext::direct())
        .await
        .unwrap();
    let value: Value = serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
    assert_eq!(value["tickets"][0]["tags"], json!([tag()]));
    {
        let requests = client.requests.lock().unwrap();
        let body: Value = serde_json::from_str(requests[0].body.as_deref().unwrap()).unwrap();
        for field in ["tag_ids", "tag_ids_any", "tag_ids_none"] {
            assert_eq!(body[field], input[field]);
        }
    }
    let detail = json!({"resource_key":"T-740", "content_digest":"content-1", "title":"Classify", "body":"Body", "state":"planning", "tags":[tag()], "events":[], "linked_objectives":[], "assignments":[], "implementation_reports":[], "merge_requests":[]});
    let client = recording(detail);
    let (_, tool) =
        workspace_ticket_read_definition(client.clone(), WorkspaceTicketReadKind::Show)();
    let output = tool
        .execute(r#"{"id":"T-740"}"#, ToolExecutionContext::direct())
        .await
        .unwrap();
    let value: Value = serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
    assert_eq!(value["tags"], json!([tag()]));
    assert_eq!(value["content_digest"], "content-1");
    let client = recording(json!({}));
    let (_, tool) =
        workspace_ticket_read_definition(client.clone(), WorkspaceTicketReadKind::Query)();
    for field in ["tag_ids", "tag_ids_any", "tag_ids_none"] {
        let input = json!({field: (0..101).map(|i| format!("tag-{i}")).collect::<Vec<_>>()});
        assert!(matches!(
            tool.execute(&input.to_string(), ToolExecutionContext::direct())
                .await,
            Err(ToolError::InvalidArgument(_))
        ));
    }
    assert!(client.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn classification_tools_reject_invalid_names_ids_and_keys_before_dispatch() {
    let client = recording(json!([]));
    for name in [" ".to_string(), "x\n".to_string(), "é".repeat(65)] {
        let (_, tool) = definition(client.clone(), TagToolKind::Create)();
        assert!(matches!(
            tool.execute(
                &json!({"name":name, "operation_key":"create"}).to_string(),
                ToolExecutionContext::direct()
            )
            .await,
            Err(ToolError::InvalidArgument(_))
        ));
    }
    for ids in [
        json!(["tag-1", "tag-1"]),
        json!([""]),
        json!((0..101).map(|i| format!("tag-{i}")).collect::<Vec<_>>()),
    ] {
        let (_, tool) = definition(client.clone(), TagToolKind::Set)();
        for field in ["tag_ids", "expected_tag_ids"] {
            let mut input = json!({"ticket":"T-740", "tag_ids":[], "expected_tag_ids":[], "operation_key":"set"});
            input[field] = ids.clone();
            assert!(matches!(
                tool.execute(&input.to_string(), ToolExecutionContext::direct())
                    .await,
                Err(ToolError::InvalidArgument(_))
            ));
        }
    }
    for (kind, mut input, _) in cases()
        .into_iter()
        .filter(|(kind, _, _)| !matches!(kind, TagToolKind::List))
    {
        input["operation_key"] = json!(" ");
        let (_, tool) = definition(client.clone(), kind)();
        assert!(matches!(
            tool.execute(&input.to_string(), ToolExecutionContext::direct())
                .await,
            Err(ToolError::InvalidArgument(_))
        ));
    }
    let (_, tool) = definition(client.clone(), TagToolKind::List)();
    assert!(
        tool.execute(r#"{"limit":201}"#, ToolExecutionContext::direct())
            .await
            .is_err()
    );
    assert!(client.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn classification_tools_require_backend_workspace_and_bound_list_results() {
    let missing = Arc::new(RecordingClient {
        requests: Mutex::new(Vec::new()),
        body: "[]".into(),
        identity: false,
        status: 200,
    });
    for (kind, input, _) in cases() {
        let (_, tool) = definition(missing.clone(), kind)();
        assert!(matches!(
            tool.execute(&input.to_string(), ToolExecutionContext::direct())
                .await,
            Err(ToolError::InvalidArgument(_))
        ));
    }
    assert!(missing.requests.lock().unwrap().is_empty());
    let client = recording(json!(vec![tag(); 201]));
    let (_, tool) = definition(client.clone(), TagToolKind::List)();
    for (input, expected) in [
        (json!({}), 200),
        (json!({"limit":2, "after":"a&b"}), 2),
        (json!({"limit":0}), 0),
    ] {
        let output = tool
            .execute(&input.to_string(), ToolExecutionContext::direct())
            .await
            .unwrap();
        let value: Value = serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
        assert_eq!(value.as_array().unwrap().len(), expected);
    }
    let requests = client.requests.lock().unwrap();
    assert_eq!(
        requests[1].path,
        "/api/w/workspace/ticket-tags?limit=2&after=a%26b"
    );
}

#[tokio::test]
async fn classification_wip_uses_normal_permissions_and_route_bound_ticket_identity() {
    let client = recording(json!([tag()]));
    let tools =
        native_ticket_tools(client.clone(), TicketFeatureAccess::workspace_authoring()).unwrap();
    let set = tools
        .iter()
        .find(|tool| tool.name == "TicketTagsSet")
        .unwrap();
    let (_, input, _) = cases().pop().unwrap();
    let permissions = Some(ToolPermissionConfig {
        default_action: ToolPermissionAction::Allow,
        rules: vec![manifest::ToolPermissionRule {
            tool: "TicketTagsSet".into(),
            action: ToolPermissionAction::Deny,
            pattern: input.to_string(),
        }],
    });
    let error = execute_native_ticket_tool(
        set,
        &permissions,
        input,
        WipCallContext {
            execution: ToolExecutionContext::direct(),
            security_context: "worker".into(),
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        WipOperationError::Protocol(ProtocolError {
            code: ProtocolErrorCode::PermissionDenied,
            ..
        })
    ));
    assert!(client.requests.lock().unwrap().is_empty());
    let handler = TicketItemWipHandler {
        tools: operation_map(
            tools
                .into_iter()
                .filter(|tool| tool.projection.surface == NativeTicketSurface::Item)
                .collect(),
        ),
        permissions: None,
        ticket_reference: "T-740".into(),
        observations: Arc::new(Mutex::new(TicketObservationState::default())),
    };
    handler
        .observations
        .lock()
        .unwrap()
        .content_digests
        .insert("T-740".into(), "content-1".into());
    let mut args = BTreeMap::from([
        ("operation_key".into(), WipValue::String("set-1".into())),
        ("tag_ids".into(), json_to_wip(&json!(["tag-1"])).unwrap()),
        ("expected_tag_ids".into(), json_to_wip(&json!([])).unwrap()),
    ]);
    assert!(
        handler
            .call(
                "tags_set",
                &args,
                WipCallContext {
                    execution: ToolExecutionContext::direct(),
                    security_context: "worker".into(),
                },
            )
            .await
            .is_ok()
    );
    assert_eq!(
        client.requests.lock().unwrap()[0].path,
        "/api/w/workspace/tickets/T-740/tags"
    );
    assert_eq!(
        handler
            .observations
            .lock()
            .unwrap()
            .content_digests
            .get("T-740")
            .map(String::as_str),
        Some("content-1")
    );
    args.insert("ticket".into(), WipValue::String("T-foreign".into()));
    assert!(
        handler
            .call(
                "tags_set",
                &args,
                WipCallContext {
                    execution: ToolExecutionContext::direct(),
                    security_context: "worker".into()
                }
            )
            .await
            .is_err()
    );
    assert_eq!(client.requests.lock().unwrap().len(), 1);
    let read_only = TicketFeatureAccess::read_only().tool_names();
    assert!(read_only.contains(&"WorkspaceTagList"));
    assert!(!read_only.contains(&"TicketTagsSet"));
}

#[tokio::test]
async fn classification_tools_preserve_backend_permission_and_stale_set_failures() {
    for status in [403, 409] {
        let client = Arc::new(RecordingClient {
            requests: Mutex::new(Vec::new()),
            body: "{}".into(),
            identity: true,
            status,
        });
        let (_, tool) = definition(client.clone(), TagToolKind::Set)();
        let (_, input, _) = cases().pop().unwrap();
        let error = tool
            .execute(&input.to_string(), ToolExecutionContext::direct())
            .await
            .unwrap_err();
        assert!(error.to_string().contains(&status.to_string()));
        assert_eq!(client.requests.lock().unwrap().len(), 1);
    }
}

#[test]
fn classification_tools_http_mutations_survive_backend_reopen_and_stale_compare() {
    use std::io::{Read, Write};
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("tags.db");
    let backend = ticket::SqliteTicketBackend::open(&db, "workspace").unwrap();
    let reference = backend.create(NewTicket::new("Classify")).unwrap();
    let initial_content_digest =
        ticket::ticket_content_digest(&backend.show(reference.id.clone().into()).unwrap());
    let ticket_id = reference.id.clone();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server_db = db.clone();
    let server = std::thread::spawn(move || {
        for _ in 0..10 {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            let mut bytes = Vec::new();
            let (headers, body) = loop {
                let mut chunk = [0u8; 8192];
                let count = stream.read(&mut chunk).unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&chunk[..count]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|v| v.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if bytes.len() >= end + 4 + length {
                        break (headers, bytes[end + 4..end + 4 + length].to_vec());
                    }
                }
            };
            let backend =
                ticket::SqliteTicketBackend::open_verified(&server_db, "workspace").unwrap();
            let line = headers.lines().next().unwrap();
            let result = if line.starts_with("POST /api/w/workspace/ticket-tags ") {
                backend
                    .mutate_workspace_tag(serde_json::from_slice(&body).unwrap())
                    .map(|tag| json!(tag))
            } else if line.starts_with(&format!("PUT /api/w/workspace/tickets/{ticket_id}/tags ")) {
                backend
                    .set_ticket_tags(
                        ticket_id.clone().into(),
                        serde_json::from_slice(&body).unwrap(),
                    )
                    .map(|tags| json!(tags))
            } else {
                assert!(
                    line.starts_with("GET /api/w/workspace/ticket-tags?"),
                    "{line}"
                );
                backend
                    .list_workspace_tags(ticket::TagListRequest::default())
                    .map(|tags| json!(tags))
            };
            let (status, response) = match result {
                Ok(value) => ("200 OK", value.to_string()),
                Err(error) => (
                    "409 Conflict",
                    json!({"message":error.to_string()}).to_string(),
                ),
            };
            write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
        }
    });
    let client: Arc<dyn WorkspaceClient> = Arc::new(crate::worker::TestWorkspaceHttpClient::new(
        "workspace",
        base,
    ));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let call = |kind, input: Value| {
        runtime.block_on(async {
            let (_, tool) = definition(client.clone(), kind)();
            tool.execute(&input.to_string(), ToolExecutionContext::direct())
                .await
        })
    };
    let first: ticket::WorkspaceTag = serde_json::from_str(
        call(
            TagToolKind::Create,
            json!({"name":"ready", "operation_key":"create-a"}),
        )
        .unwrap()
        .content
        .as_deref()
        .unwrap(),
    )
    .unwrap();
    let second: ticket::WorkspaceTag = serde_json::from_str(
        call(
            TagToolKind::Create,
            json!({"name":"closed", "operation_key":"create-b"}),
        )
        .unwrap()
        .content
        .as_deref()
        .unwrap(),
    )
    .unwrap();
    let set = json!({"ticket":reference.id, "expected_tag_ids":[], "tag_ids":[first.id, second.id], "operation_key":"set-both"});
    let receipt = call(TagToolKind::Set, set.clone()).unwrap();
    assert_eq!(
        call(TagToolKind::Set, set).unwrap().content,
        receipt.content
    );
    assert!(call(TagToolKind::Set, json!({"ticket":reference.id, "expected_tag_ids":[], "tag_ids":[], "operation_key":"stale"})).is_err());
    let removed = call(TagToolKind::Set, json!({"ticket":reference.id, "expected_tag_ids":[second.id, first.id], "tag_ids":[], "operation_key":"remove-both"})).unwrap();
    assert_eq!(removed.content.as_deref(), Some("[]"));
    assert!(
        backend
            .show(reference.id.clone().into())
            .unwrap()
            .tags
            .is_empty()
    );
    call(TagToolKind::Set, json!({"ticket":reference.id, "expected_tag_ids":[], "tag_ids":[first.id, second.id], "operation_key":"reattach-both"})).unwrap();
    call(TagToolKind::Rename, json!({"tag_id":first.id, "expected_last_operation_key":first.last_operation_key, "name":"renamed", "operation_key":"rename-a"})).unwrap();
    call(TagToolKind::Delete, json!({"tag_id":second.id, "expected_last_operation_key":second.last_operation_key, "operation_key":"delete-b"})).unwrap();
    let listed = call(TagToolKind::List, json!({})).unwrap();
    let tags: Vec<ticket::WorkspaceTag> =
        serde_json::from_str(listed.content.as_deref().unwrap()).unwrap();
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[0].name, "renamed");
    server.join().unwrap();
    let reopened = ticket::SqliteTicketBackend::open_verified(&db, "workspace").unwrap();
    let ticket = reopened.show(reference.id.into()).unwrap();
    assert_eq!(ticket.tags, tags);
    assert_eq!(
        ticket::ticket_content_digest(&ticket),
        initial_content_digest
    );
    assert_eq!(ticket.meta.workflow_state, TicketWorkflowState::Planning);
    assert_eq!(
        ticket
            .events
            .iter()
            .filter(|event| event.kind == ticket::TicketEventKind::TagsChanged)
            .count(),
        4,
        "replay and stale input must not append membership events"
    );
}
