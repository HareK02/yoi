//! Connection tests run the production model-facing Inspect/Invoke, Host, Client, native result
//! validation and domain dispatch; only Workspace HTTP is scripted. They do not assert Backend
//! transactions or persistence. Unit cases pin the explicit response-field selection boundary.

use super::super::tests::{RecordingWorkspaceClient, response};
use super::*;
use crate::wip::{WipRuntime, install_wip_mode_with_mounts};
use agen::Engine;
use agen::llm_client::client::LlmClient;
use agen::llm_client::{ClientError, Request as LlmRequest, ResponseStream};
use agen::state::Mutable;

#[derive(Clone)]
struct NoLlm;
#[async_trait]
impl LlmClient for NoLlm {
    async fn stream(&self, _: LlmRequest) -> Result<ResponseStream, ClientError> {
        panic!("the model-facing tools are driven directly")
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
    fn new(registry: WipMountRegistry) -> Self {
        let mut engine = Engine::new_annotated(NoLlm);
        let runtime =
            install_wip_mode_with_mounts(&mut engine, None, "mr-model-tools".into(), registry)
                .unwrap();
        engine.tool_server_handle().flush_pending();
        Self { engine, runtime }
    }
    async fn execute(&self, name: &str, input: Value) -> Result<Value, ToolError> {
        let output = self
            .engine
            .tool_server_handle()
            .call_tool(name, &input.to_string(), ToolExecutionContext::direct())
            .await
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        Ok(serde_json::from_str(&output.content.unwrap()).unwrap())
    }
    async fn inspect(&self, path: &Value) -> Value {
        self.execute("Inspect", json!({"path":path})).await.unwrap()
    }
    async fn invoke(
        &self,
        object: &Value,
        operation: &str,
        arguments: Value,
    ) -> Result<Value, ToolError> {
        self.execute("Invoke", json!({"path":object["path"], "interface":object["interfaces"][0]["reference"], "operation":operation, "arguments":arguments})).await
    }
}

fn projected(kind: Kind, data: Value, published: bool) -> Value {
    crate::wip::wip_to_json(
        &value(
            kind,
            data,
            "MR-1",
            "/changes",
            published.then_some("/cases"),
        )
        .unwrap_or_else(|_| panic!("result mapping")),
    )
    .unwrap()
}

#[test]
fn explicit_domain_reference_fields_preserve_full_snapshot_evidence_without_recursive_inference() {
    let snapshot = json!([
        {"merge_request_id":"MR-1", "subject_ref":"source-1"},
        {"merge_request_id":"MR-2", "subject_ref":"source-2"},
        {"merge_request_id":"MR-2", "subject_ref":"older-source-2"},
        {"merge_request_id":"unroutable/id", "subject_ref":"source-3"}
    ]);
    let event = json!({"kind":"review", "event_id":"event-1", "subject_ref":"source-1",
        "ticket_content_digest":"digest", "ticket_merge_request_subjects":snapshot,
        "body":"/cases/T-FAKE", "findings":[{"path":"/changes/MR-FAKE"}]});
    let data = json!({"merge_request_id":"MR-1", "ticket_ids":["ticket-internal", "T-2"],
        "linked_tickets":[{"ticket_id":"ticket-internal", "key":"T-1"}, {"ticket_id":"T-3","key":null}],
        "thread":[event, {"kind":"comment", "ticket_merge_request_subjects":[{"merge_request_id":"MR-FAKE"}]}],
        "state":"open", "page":{"next_cursor":"cursor-MR-FAKE"},
        "body":{"merge_request_id":"MR-FAKE","ticket_id":"T-FAKE"}});
    let result = projected(Kind::Show, data.clone(), true);
    assert_eq!(result["data"], data);
    assert_eq!(result["entry"], "/changes/MR-1");
    assert_eq!(
        result["links"]["tickets"],
        json!([
            {"id":"T-2","entry":"/cases/T-2"},
            {"id":"T-3","entry":"/cases/T-3"},
            {"id":"ticket-internal"}
        ])
    );
    assert_eq!(
        result["links"]["merge_requests"],
        json!([
            {"id":"MR-1","entry":"/changes/MR-1"},
            {"id":"MR-2","entry":"/changes/MR-2"},
            {"id":"unroutable/id"}
        ])
    );
    for (kind, response) in [
        (Kind::Readiness, json!({"ready":false, "review":event})),
        (Kind::Review, event),
    ] {
        let result = projected(kind, response.clone(), false);
        assert_eq!(result["data"], response);
        assert_eq!(
            result["links"]["merge_requests"].as_array().unwrap().len(),
            3
        );
    }
}

#[test]
fn absent_null_and_empty_references_have_empty_lists_and_unpublished_entries_are_omitted() {
    for kind in ALL_KINDS {
        for data in [
            json!({}),
            json!({"review":null, "thread":[], "ticket_ids":[], "linked_tickets":[]}),
        ] {
            let result = projected(kind, data.clone(), false);
            assert_eq!(result["data"], data);
            assert_eq!(result["links"], json!({"tickets":[],"merge_requests":[]}));
        }
    }
    let result = projected(Kind::Open, json!({"ticket_ids":["T-1"]}), false);
    assert_eq!(result["links"]["tickets"], json!([{"id":"T-1"}]));
}

fn registry(client: Arc<dyn WorkspaceClient>, publish_ticket: bool) -> WipMountRegistry {
    registry_with_order(client, publish_ticket, false)
}

fn registry_with_order(
    client: Arc<dyn WorkspaceClient>,
    publish_ticket: bool,
    ticket_first: bool,
) -> WipMountRegistry {
    let mut registry = WipMountRegistry::new();
    let ticket = registry.allocate_namespace("ticket", "cases").unwrap();
    let mr = registry
        .allocate_namespace("merge-request", "changes")
        .unwrap();
    let mount_ticket = |registry: &mut WipMountRegistry| {
        crate::feature::builtin::ticket::mount_workspace_http_ticket_wip(
            registry,
            client.clone(),
            crate::feature::builtin::ticket::TicketFeatureAccess::read_only(),
            None,
            &ticket,
        )
        .unwrap();
    };
    if publish_ticket && ticket_first {
        mount_ticket(&mut registry);
    }
    mount_workspace_http_merge_request_wip(
        &mut registry,
        client.clone(),
        MergeRequestFeatureConfig {
            show: true,
            open: true,
            readiness_check: true,
            ..Default::default()
        },
        None,
        &mr,
    )
    .unwrap();
    if publish_ticket && !ticket_first {
        mount_ticket(&mut registry);
    }
    registry
}

async fn inspect(runtime: &WipRuntime, path: &str) -> Value {
    serde_json::from_str(
        &runtime
            .inspect(path.into(), false)
            .await
            .unwrap()
            .content
            .unwrap(),
    )
    .unwrap()
}

async fn invoke(
    runtime: &WipRuntime,
    object: &Value,
    operation: &str,
    arguments: Value,
) -> Result<Value, ToolError> {
    let interface = &object["interfaces"][0]["reference"];
    runtime
        .invoke(
            object["path"].as_str().unwrap().into(),
            wip_protocol::InterfaceReference {
                scope: interface["scope"].as_str().unwrap().into(),
                name: interface["name"].as_str().unwrap().into(),
            },
            operation.into(),
            arguments,
            ToolExecutionContext::direct(),
        )
        .await
        .map(|output| serde_json::from_str(&output.content.unwrap()).unwrap())
}

#[tokio::test]
async fn inspect_open_entry_read_and_linked_ticket_use_published_contract_without_prefetch_or_indexing()
 {
    for ticket_first in [true, false] {
        let detail = json!({"resource_key":"T-1", "title":"Intent", "body":"Requirements", "body_truncated":false,
        "content_digest":"observed", "state":"open", "targets":[], "events":[], "linked_objectives":[],
        "initial_post":{"sequence":0,"event_ref":"initial","is_initial":true,"kind":"comment","body":"Requirements","body_truncated":false,"body_digest":"a".repeat(64),"editable":true,"author":null,"at":null,"edited_at":null,"edited_by":null},
        "closure_operation_key":null,"current_closure":null,"legacy_handoff":null,"closure_history":[],
        "assignment_tracking":[],"tags":[],"assignments":[],"implementation_reports":[],"merge_requests":[]});
        let page = json!({"merge_request_id":"MR-1", "ticket_ids":["T-1"], "linked_tickets":[{"ticket_id":"T-1","key":"T-1"}],
        "thread":[{"kind":"review_requested", "subject_ref":"sha-1", "ticket_merge_request_subjects":[
            {"merge_request_id":"MR-1","subject_ref":"sha-1"}, {"merge_request_id":"MR-2","subject_ref":"sha-2"}]}]});
        let client = Arc::new(RecordingWorkspaceClient::new(vec![
            response(json!({"merge_request_id":"MR-1", "ticket_ids":["T-1"]})),
            response(page.clone()),
            response(detail),
            response(json!({"ready":false,"review":null})),
        ]));
        let model = Model::new(registry_with_order(client.clone(), true, ticket_first));
        let runtime = &model.runtime;
        let collection = model.inspect(&json!("/changes")).await;
        let definition = model.inspect(&collection["interfaces"][0]["path"]).await;
        let signature = definition["interface_signature"].as_str().unwrap();
        assert!(signature.contains("`entry`: entry"), "{signature}");
        assert!(signature.contains("data: json"), "{signature}");
        assert!(signature.contains("`entry`?: entry"), "{signature}");
        assert!(client.requests.lock().unwrap().is_empty());
        let before = runtime.metrics();
        let opened = model.invoke(&collection, "open", json!({"ticket":"/cases/T-1", "repository":{"key":"main","source":"work","target":"develop"}})).await.unwrap();
        assert_eq!(opened["entry"], "/changes/MR-1");
        assert_eq!(client.requests.lock().unwrap().len(), 1);
        assert_eq!(
            runtime.metrics().inspect_round_trips,
            before.inspect_round_trips,
            "Entry values must not be inspected eagerly"
        );
        let tree = runtime
            .tree("/changes".into(), 3, false)
            .await
            .unwrap()
            .content
            .unwrap();
        assert!(!tree.contains("MR-1"), "dynamic items remain nonindexable");
        let item = model.inspect(&opened["entry"]).await;
        let item_definition = model.inspect(&item["interfaces"][0]["path"]).await;
        assert!(
            item_definition["interface_signature"]
                .as_str()
                .unwrap()
                .contains("`entry`: entry")
        );
        assert_eq!(
            client.requests.lock().unwrap().len(),
            1,
            "Inspect must not read domain data"
        );
        let read = model.invoke(&item,
        "read",
        json!({"page":{"after":{"$case":"value","value":7},"limit":{"$case":"value","value":25}}}),
    )
    .await
    .unwrap();
        assert_eq!(read["data"], page);
        assert_eq!(read["links"]["merge_requests"][1]["entry"], "/changes/MR-2");
        assert_eq!(
            client.requests.lock().unwrap().len(),
            2,
            "linked snapshot must not be prefetched"
        );
        let ticket = model.inspect(&read["links"]["tickets"][0]["entry"]).await;
        let ticket_definition = model.inspect(&ticket["interfaces"][0]["path"]).await;
        assert!(
            ticket_definition["interface_signature"]
                .as_str()
                .unwrap()
                .contains("operation read(")
        );
        let ticket_read = model.invoke(&ticket, "read", json!({})).await.unwrap();
        assert!(ticket_read.to_string().contains("Intent"));
        // Reading updates the MR validator: refresh the observation for this separate operation.
        runtime
            .inspect(read["entry"].as_str().unwrap().into(), true)
            .await
            .unwrap();
        let readiness = model.invoke(&item, "readiness", json!({})).await.unwrap();
        assert_eq!(readiness["entry"], read["entry"]);
        assert_eq!(
            readiness["links"],
            json!({"tickets":[],"merge_requests":[]})
        );
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 4);
        assert_eq!(
            requests[1].path,
            "/api/w/ws/merge-requests/MR-1?after=7&limit=25"
        );
        assert!(requests[2].path.contains("/tickets/T-1"));
    }
}

#[tokio::test]
async fn selected_denied_or_deleted_entry_never_retries_successful_open() {
    for status in [403, 404] {
        let client = Arc::new(RecordingWorkspaceClient::new(vec![
            response(json!({"merge_request_id":"MR-1","ticket_ids":["T-1"]})),
            crate::worker::WorkspaceResponse {
                status,
                body: "{}".into(),
            },
        ]));
        let runtime =
            WipRuntime::from_mounts(registry(client.clone(), true), format!("negative-{status}"))
                .unwrap();
        let collection = inspect(&runtime, "/changes").await;
        let opened = invoke(&runtime, &collection, "open", json!({"ticket":"/cases/T-1","repository":{"key":"main","source":"work","target":"develop"}})).await.unwrap();
        let ticket = inspect(
            &runtime,
            opened["links"]["tickets"][0]["entry"].as_str().unwrap(),
        )
        .await;
        let failure = invoke(&runtime, &ticket, "read", json!({}))
            .await
            .unwrap_err();
        assert!(
            failure.to_string().contains(if status == 403 {
                "PermissionDenied"
            } else {
                "NotFound"
            }),
            "{failure}"
        );
        assert_eq!(opened["data"]["merge_request_id"], "MR-1");
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.path.ends_with("/merge-request"))
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn unpublished_ticket_reference_omits_entry_but_open_remains_successful() {
    let client = Arc::new(RecordingWorkspaceClient::new(vec![response(
        json!({"merge_request_id":"MR-1","ticket_ids":["T-1"]}),
    )]));
    let runtime =
        WipRuntime::from_mounts(registry(client.clone(), false), "unpublished-result".into())
            .unwrap();
    let collection = inspect(&runtime, "/changes").await;
    let opened = invoke(&runtime, &collection, "open", json!({"ticket":"/cases/T-1","repository":{"key":"main","source":"work","target":"develop"}})).await.unwrap();
    assert_eq!(opened["links"]["tickets"], json!([{"id":"T-1"}]));
    assert!(runtime.inspect("/cases/T-1".into(), false).await.is_err());
    assert_eq!(client.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn successful_open_with_unroutable_response_identity_is_unknown_without_retry() {
    let client = Arc::new(RecordingWorkspaceClient::new(vec![response(json!({
        "merge_request_id":"not/a/route", "ticket_ids":["T-1"]
    }))]));
    let model = Model::new(registry(client.clone(), true));
    let collection = model.inspect(&json!("/changes")).await;
    let failure = model.invoke(&collection, "open", json!({"ticket":"/cases/T-1", "repository":{"key":"main","source":"work","target":"develop"}})).await.unwrap_err();
    assert!(
        failure
            .to_string()
            .contains("outcome unknown; do not retry automatically"),
        "{failure}"
    );
    assert_eq!(client.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn malformed_domain_response_after_open_is_unknown_without_replaying_mutation() {
    let client = Arc::new(RecordingWorkspaceClient::new(vec![
        crate::worker::WorkspaceResponse {
            status: 200,
            body: "not-json".into(),
        },
    ]));
    let model = Model::new(registry(client.clone(), true));
    let collection = model.inspect(&json!("/changes")).await;
    let failure = model.invoke(&collection, "open", json!({"ticket":"/cases/T-1", "repository":{"key":"main","source":"work","target":"develop"}})).await.unwrap_err();
    assert!(
        failure
            .to_string()
            .contains("outcome unknown; do not retry automatically"),
        "{failure}"
    );
    assert_eq!(client.requests.lock().unwrap().len(), 1);
}

#[test]
fn shared_domain_value_and_ordinary_tool_boundary_preserve_the_same_response_data() {
    let data = json!({"merge_request_id":"MR-1","ticket_ids":["T-1"],"body":"Do not infer /cases/T-9", "thread":[{"kind":"review", "ticket_merge_request_subjects":[{"merge_request_id":"MR-2","subject_ref":"source-id"}]}]});
    let client = Arc::new(RecordingWorkspaceClient::new(vec![
        response(data.clone()),
        response(data.clone()),
    ]));
    let read = || command::Command::Read {
        merge_request_id: "MR-1".into(),
        after: None,
        limit: Some(25),
    };
    let domain = command::execute_value(client.clone(), read()).unwrap();
    assert_eq!(domain, data);
    let tool = command::execute(client.clone(), read()).unwrap();
    assert_eq!(tool.summary, "ShowMergeRequest");
    assert_eq!(
        serde_json::from_str::<Value>(&tool.content.unwrap()).unwrap(),
        domain
    );
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path, requests[1].path);
    assert_eq!(client.authorizations.lock().unwrap().len(), 2);
}
