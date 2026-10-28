use super::*;
use crate::worker::{WorkspaceClientError, WorkspaceResponse};

#[derive(Debug)]
struct RecordingClient {
    requests: Mutex<Vec<WorkspaceRequest>>,
    authorizations: Mutex<std::collections::VecDeque<Value>>,
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
        if request.path == "/api/w/workspace/authorization/check" {
            let body: Value = serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
            assert_eq!(Some(body), self.authorizations.lock().unwrap().pop_front());
            return Ok(WorkspaceResponse {
                status: 200,
                body: json!({"scope": null}).to_string(),
            });
        }
        self.requests.lock().unwrap().push(request);
        Ok(WorkspaceResponse {
            status: self.status,
            body: self.body.clone(),
        })
    }
}
fn expected_authorization(operation: &str, ticket_id: Option<&str>) -> Value {
    let resource = match ticket_id {
        Some(ticket_id) => {
            json!({"kind":"ticket", "workspace_id":"workspace", "ticket_id":ticket_id})
        }
        None => json!({"kind":"workspace", "workspace_id":"workspace"}),
    };
    json!({"operation":operation, "resource":resource})
}
fn recording(body: Value, expected: impl IntoIterator<Item = Value>) -> Arc<RecordingClient> {
    Arc::new(RecordingClient {
        requests: Mutex::new(Vec::new()),
        authorizations: Mutex::new(expected.into_iter().collect()),
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
        let client = recording(json!([]), []);
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
    let item = json!({"resource_key":"T-740", "title":"Classify", "state":"open", "closure_operation_key":null, "current_closure":null, "legacy_handoff":null, "closure_history":[], "assignment_tracking":[], "tags":[tag()], "matched_fields":[], "linked_objective_keys":[], "relation_count":0, "blocker_count":0, "unresolved_blocker_count":0, "unresolved_review_count":0, "merge_requests":[]});
    let client = recording(
        json!({"items":[item], "page":{"has_more":false, "next_cursor":null}}),
        [expected_authorization("ticket.query", None)],
    );
    let (meta, tool) =
        workspace_ticket_read_definition(client.clone(), WorkspaceTicketReadKind::Query)();
    for field in ["tag_ids", "tag_ids_any", "tag_ids_none"] {
        assert_eq!(meta.input_schema["properties"][field]["maxItems"], 100);
    }
    let input = json!({"tag_ids":["tag-1"], "tag_ids_any":["tag-2", "tag-3"], "tag_ids_none":["tag-4"], "states":["open"], "limit":7});
    let output = tool
        .execute(&input.to_string(), ToolExecutionContext::direct())
        .await
        .unwrap();
    assert!(client.authorizations.lock().unwrap().is_empty());
    let value: Value = serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
    assert_eq!(value["tickets"][0]["tags"], json!([tag()]));
    assert_eq!(value["tickets"][0]["state"], "open");
    for field in ["closure_operation_key", "current_closure", "legacy_handoff"] {
        assert!(
            value["tickets"][0].as_object().unwrap().contains_key(field),
            "{field}"
        );
        assert_eq!(value["tickets"][0][field], Value::Null);
    }
    {
        let requests = client.requests.lock().unwrap();
        let body: Value = serde_json::from_str(requests[0].body.as_deref().unwrap()).unwrap();
        for field in ["states", "tag_ids", "tag_ids_any", "tag_ids_none"] {
            assert_eq!(body[field], input[field]);
        }
    }
    let detail = json!({"resource_key":"T-740", "content_digest":"content-1", "title":"Classify", "body":"Body", "body_truncated":false, "targets":[], "initial_post":{"sequence":0,"event_ref":"initial","is_initial":true,"kind":"comment","body":"Body","body_truncated":false,"body_digest":"a".repeat(64),"editable":true,"author":null,"at":null,"edited_at":null,"edited_by":null}, "state":"open", "closure_operation_key":null, "current_closure":null, "legacy_handoff":null, "closure_history":[], "assignment_tracking":[], "tags":[tag()], "events":[], "linked_objectives":[], "assignments":[], "implementation_reports":[], "merge_requests":[]});
    let client = recording(
        detail,
        [expected_authorization("ticket.read", Some("T-740"))],
    );
    let (_, tool) =
        workspace_ticket_read_definition(client.clone(), WorkspaceTicketReadKind::Show)();
    let output = tool
        .execute(r#"{"id":"T-740"}"#, ToolExecutionContext::direct())
        .await
        .unwrap();
    assert!(client.authorizations.lock().unwrap().is_empty());
    let value: Value = serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
    assert_eq!(value["tags"], json!([tag()]));
    assert_eq!(value["content_digest"], "content-1");
    assert_eq!(value["state"], "open");
    assert_eq!(value["closure_history"], json!([]));
    for field in ["closure_operation_key", "current_closure", "legacy_handoff"] {
        assert!(value.as_object().unwrap().contains_key(field), "{field}");
        assert_eq!(value[field], Value::Null);
    }
    let client = recording(json!({}), []);
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
    for state in [
        "planning",
        "ready",
        "queued",
        "inprogress",
        "done",
        "invalid",
    ] {
        let input = json!({"states":[state], "tag_ids":["tag-1"]});
        assert!(
            matches!(
                tool.execute(&input.to_string(), ToolExecutionContext::direct())
                    .await,
                Err(ToolError::InvalidArgument(_))
            ),
            "{state}"
        );
    }
    assert!(client.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn classification_tools_reject_invalid_names_ids_and_keys_before_dispatch() {
    let client = recording(json!([]), []);
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
        authorizations: Mutex::new(Default::default()),
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
    let client = recording(
        json!(vec![tag(); 201]),
        (0..3).map(|_| expected_authorization("ticket.tag.list", None)),
    );
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
    assert!(client.authorizations.lock().unwrap().is_empty());
    let requests = client.requests.lock().unwrap();
    assert_eq!(
        requests[1].path,
        "/api/w/workspace/ticket-tags?limit=2&after=a%26b"
    );
}

#[tokio::test]
async fn classification_wip_uses_normal_permissions_and_route_bound_ticket_identity() {
    let client = recording(
        json!([tag()]),
        [expected_authorization("ticket.tags.set", Some("T-740"))],
    );
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
            result_namespaces: Default::default(),
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
        collection_route: "/tickets".into(),
        merge_request_route: "/merge-requests".into(),
        objective_route: "/objectives".into(),
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
        ("tags".into(), json_to_wip(&json!(["tag-1"])).unwrap()),
        ("observed".into(), json_to_wip(&json!({"tags":[]})).unwrap()),
    ]);
    assert!(
        handler
            .call(
                "tags_set",
                &args,
                WipCallContext {
                    result_namespaces: Default::default(),
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
                    result_namespaces: Default::default(),
                    execution: ToolExecutionContext::direct(),
                    security_context: "worker".into()
                }
            )
            .await
            .is_err()
    );
    assert!(client.authorizations.lock().unwrap().is_empty());
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
            authorizations: Mutex::new(
                [expected_authorization("ticket.tags.set", Some("T-740"))]
                    .into_iter()
                    .collect(),
            ),
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
        assert!(client.authorizations.lock().unwrap().is_empty());
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
        let mut authorizations = std::collections::VecDeque::from([
            expected_authorization("ticket.tag.create", None),
            expected_authorization("ticket.tag.create", None),
            expected_authorization("ticket.tags.set", Some(&ticket_id)),
            expected_authorization("ticket.tags.set", Some(&ticket_id)),
            expected_authorization("ticket.tags.set", Some(&ticket_id)),
            expected_authorization("ticket.tags.set", Some(&ticket_id)),
            expected_authorization("ticket.tags.set", Some(&ticket_id)),
            expected_authorization("ticket.tag.rename", None),
            expected_authorization("ticket.tag.delete", None),
            expected_authorization("ticket.tag.list", None),
        ]);
        for _ in 0..20 {
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
            let result = if line.starts_with("POST /api/w/workspace/authorization/check ") {
                let actual: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(Some(actual), authorizations.pop_front());
                Ok(json!({"scope": null}))
            } else if line.starts_with("POST /api/w/workspace/ticket-tags ") {
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
        assert!(authorizations.is_empty());
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
    assert_eq!(ticket.meta.workflow_state, TicketState::Open);
    assert!(ticket.current_closure.is_none());
    assert!(ticket.closure_operation_key.is_none());
    assert!(ticket.case_history.is_empty());
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
