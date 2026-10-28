use super::*;
use crate::worker::{WorkspaceClientError, WorkspaceRequest, WorkspaceResponse};
use serde::Serialize;
use std::collections::VecDeque;

#[derive(Debug)]
struct Client {
    auth_denied: std::sync::atomic::AtomicBool,
    denied_resource: Mutex<Option<server_api::authorization::PermissionResource>>,
    authorizations: Mutex<Vec<server_api::authorization::OperationAuthorizationRequest>>,
    responses: Mutex<VecDeque<Result<WorkspaceResponse, WorkspaceClientError>>>,
    requests: Mutex<Vec<WorkspaceRequest>>,
    capabilities: Mutex<WorkspaceResponse>,
}
impl Client {
    fn new(responses: Vec<WorkspaceResponse>) -> Arc<Self> {
        Arc::new(Self {
            auth_denied: std::sync::atomic::AtomicBool::new(false),
            denied_resource: Mutex::new(None),
            authorizations: Mutex::new(Vec::new()),
            responses: Mutex::new(responses.into_iter().map(Ok).collect()),
            requests: Mutex::new(Vec::new()),
            capabilities: Mutex::new(response(DriveCapabilities { write: false })),
        })
    }
}
impl WorkspaceClient for Client {
    fn workspace_id(&self) -> Option<&str> {
        Some("workspace")
    }
    fn kind(&self) -> &str {
        "drive-test"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn execute(
        &self,
        request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        if request.path.ends_with("/authorization/check") {
            let input: server_api::authorization::OperationAuthorizationRequest =
                serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
            self.authorizations.lock().unwrap().push(input.clone());
            if self.auth_denied.load(std::sync::atomic::Ordering::SeqCst)
                || self.denied_resource.lock().unwrap().as_ref() == Some(&input.resource)
            {
                return Ok(WorkspaceResponse {
                    status: 403,
                    body: "{}".into(),
                });
            }
            return Ok(WorkspaceResponse {
                status: 200,
                body: serde_json::json!({"scope": input.resource}).to_string(),
            });
        }
        let capabilities = request.path == "/api/w/workspace/drive/capabilities";
        self.requests.lock().unwrap().push(request);
        if capabilities {
            let response = self.capabilities.lock().unwrap();
            return Ok(WorkspaceResponse {
                status: response.status,
                body: response.body.clone(),
            });
        }
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected Backend call")
    }
}
fn response(value: impl Serialize) -> WorkspaceResponse {
    WorkspaceResponse {
        status: 200,
        body: serde_json::to_string(&value).unwrap(),
    }
}
fn entry(id: &str, last_mutation_id: &str) -> DriveEntry {
    DriveEntry {
        entry: DriveEntryRef {
            workspace_id: "workspace".into(),
            node_id: id.into(),
        },
        parent: Some(DriveEntryRef {
            workspace_id: "workspace".into(),
            node_id: "1".into(),
        }),
        name: "note.md".into(),
        kind: DriveEntryKind::File,
        last_mutation_id: last_mutation_id.into(),
        size: Some(5),
        content_type: Some("text/markdown".into()),
        updated_by: "worker".into(),
        updated_at: "now".into(),
        latest_url: format!("/api/w/workspace/drive/download?id={id}"),
    }
}
fn feature(client: Arc<Client>) -> DriveFeature {
    DriveFeature::new(client, Arc::new(WorkdirSessionRouter::new()))
}
async fn execute(
    feature: &DriveFeature,
    name: &str,
    input: Value,
) -> Result<DriveOutput, DriveError> {
    feature
        .execute(
            name,
            &input.to_string(),
            &ToolExecutionContext::new(name, "batch", 0),
            None,
        )
        .await
}

#[test]
fn drive_feature_activation_never_creates_authority_and_schemas_hide_last_mutation_ids() {
    let client = Client::new(Vec::new());
    let router = Arc::new(WorkdirSessionRouter::new());
    assert!(DriveFeature::configured(client.clone(), false, router.clone()).is_none());
    assert!(
        DriveFeature::configured(
            crate::worker::unavailable_workspace_client(None, "standalone"),
            true,
            router
        )
        .is_none()
    );
    let feature = feature(client);
    for (name, definition) in feature.tools() {
        let (meta, _) = definition();
        assert_eq!(name, meta.name);
        let schema = meta.input_schema.to_string();
        assert!(!schema.contains("expected_mutation_id"), "{name}");
        assert!(!schema.contains("grant_id"), "{name}");
    }
    assert_eq!(feature.descriptor().instructions.len(), 1);
}

#[tokio::test]
async fn foreign_or_unobserved_entry_cannot_dispatch_mutation() {
    let client = Client::new(Vec::new());
    let feature = feature(client.clone());
    let own = json!({"workspace_id":"workspace","node_id":"9007199254740993"});
    assert!(matches!(
        execute(&feature, "DriveWrite", json!({"entry":own,"content":"new"})).await,
        Err(DriveError::Invalid(_))
    ));
    assert!(matches!(
        execute(
            &feature,
            "DriveDelete",
            json!({"entry":{"workspace_id":"other","node_id":"1"}})
        )
        .await,
        Err(DriveError::Denied)
    ));
    for id in [json!(1), json!("01"), json!("9223372036854775808")] {
        assert!(matches!(
            execute(
                &feature,
                "DriveMetadata",
                json!({"entry":{"workspace_id":"workspace","node_id":id}})
            )
            .await,
            Err(DriveError::Invalid(_))
        ));
    }
    assert!(client.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn stale_edit_returns_conflict_without_upgrading_last_mutation_id_or_dispatching_write() {
    let old = entry("2", "1");
    let current = entry("2", "2");
    let client = Client::new(vec![response(DriveReadTextResponse {
        entry: current,
        text: "hello".into(),
        truncated: false,
    })]);
    let feature = feature(client.clone());
    feature.observe(old.clone()).unwrap();
    let result = execute(
        &feature,
        "DriveEdit",
        json!({"entry":old.entry,"old_string":"hello","new_string":"new"}),
    )
    .await;
    assert!(matches!(result, Err(DriveError::Conflict)));
    assert_eq!(client.requests.lock().unwrap().len(), 1);
    assert_eq!(
        feature.observed(&old.entry, None).unwrap().last_mutation_id,
        "1"
    );
}

#[tokio::test]
async fn shared_edit_rules_reject_ambiguous_and_truncated_preimages_without_mutation() {
    for (text, truncated) in [("hello hello", false), ("hello", true)] {
        let old = entry("2", "1");
        let client = Client::new(vec![response(DriveReadTextResponse {
            entry: old.clone(),
            text: text.into(),
            truncated,
        })]);
        let feature = feature(client.clone());
        feature.observe(old.clone()).unwrap();
        assert!(
            execute(
                &feature,
                "DriveEdit",
                json!({"entry":old.entry,"old_string":"hello","new_string":"new"})
            )
            .await
            .is_err()
        );
        assert!(
            client
                .requests
                .lock()
                .unwrap()
                .iter()
                .all(|r| r.method == crate::worker::WorkspaceRequestMethod::Get)
        );
    }
}

#[tokio::test]
async fn successful_mutation_uses_observed_committed_request_id_and_requires_next_explicit_observation()
 {
    let old = entry("9007199254740993", "9007199254740993");
    let updated = entry("9007199254740993", "9007199254740994");
    let client = Client::new(Vec::new());
    let feature = feature(client.clone());
    feature.observe(old.clone()).unwrap();
    let context = ToolExecutionContext::new("write", "batch", 0);
    let id = feature.request_id(&context);
    client
        .responses
        .lock()
        .unwrap()
        .push_back(Ok(response(DriveMutationResponse {
            request_id: id,
            entry: Some(updated),
        })));
    feature
        .execute(
            "DriveWrite",
            &json!({"entry":old.entry,"content":"new"}).to_string(),
            &context,
            None,
        )
        .await
        .unwrap();
    let requests = client.requests.lock().unwrap();
    let body: Value = serde_json::from_str(requests[0].body.as_ref().unwrap()).unwrap();
    assert_eq!(body["mutation"]["expected_mutation_id"], "9007199254740993");
    assert_eq!(body["mutation"]["id"]["node_id"], "9007199254740993");
    drop(requests);
    assert!(feature.observed(&old.entry, None).is_err());
}

#[tokio::test]
async fn lost_mutation_response_queries_same_receipt_without_resending() {
    let old = entry("2", "1");
    let client = Client::new(Vec::new());
    let feature = feature(client.clone());
    feature.observe(old.clone()).unwrap();
    let context = ToolExecutionContext::new("write", "batch", 0);
    let id = feature.request_id(&context);
    client.responses.lock().unwrap().extend([
        Err(WorkspaceClientError::Request("lost".into())),
        Ok(response(DriveRequestStatusResponse {
            request_id: id.clone(),
            state: DriveRequestState::Committed,
            response: Some(DriveMutationResponse {
                request_id: id.clone(),
                entry: Some(entry("2", "2")),
            }),
        })),
    ]);
    feature
        .execute(
            "DriveWrite",
            &json!({"entry":old.entry,"content":"new"}).to_string(),
            &context,
            None,
        )
        .await
        .unwrap();
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].path.ends_with(&format!("/requests/{id}")));
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.method == crate::worker::WorkspaceRequestMethod::Post)
            .count(),
        1
    );
}

#[tokio::test]
async fn uncommitted_receipt_snapshot_does_not_turn_unknown_into_failure_or_retry() {
    let old = entry("2", "1");
    let client = Client::new(Vec::new());
    let feature = feature(client.clone());
    feature.observe(old.clone()).unwrap();
    let context = ToolExecutionContext::new("write", "batch", 0);
    let id = feature.request_id(&context);
    client.responses.lock().unwrap().extend([
        Err(WorkspaceClientError::Request("lost".into())),
        Ok(response(DriveRequestStatusResponse {
            request_id: id.clone(),
            state: DriveRequestState::Uncommitted,
            response: None,
        })),
    ]);
    let result = feature
        .execute(
            "DriveWrite",
            &json!({"entry":old.entry,"content":"new"}).to_string(),
            &context,
            None,
        )
        .await;
    assert!(matches!(result,Err(DriveError::OutcomeUnknown(ref observed)) if observed==&id));
    assert_eq!(client.requests.lock().unwrap().len(), 2);
}

fn invalid_completions(id: &str) -> Vec<DriveMutationResponse> {
    let mut invalid_entry = entry("2", "2");
    invalid_entry.last_mutation_id = "\ninvalid-request-id".into();
    vec![
        DriveMutationResponse {
            request_id: "another-request".into(),
            entry: Some(entry("2", "2")),
        },
        DriveMutationResponse {
            request_id: id.into(),
            entry: Some(invalid_entry),
        },
    ]
}

#[tokio::test]
async fn invalid_mutation_completion_recovers_from_original_receipt_without_resending() {
    let context = ToolExecutionContext::new("write", "batch", 0);
    let client = Client::new(vec![]);
    let feature = feature(client.clone());
    let id = feature.request_id(&context);
    for completion in invalid_completions(&id) {
        let committed = DriveMutationResponse {
            request_id: id.clone(),
            entry: Some(entry("2", "2")),
        };
        client.requests.lock().unwrap().clear();
        client.responses.lock().unwrap().extend([
            Ok(response(completion)),
            Ok(response(DriveRequestStatusResponse {
                request_id: id.clone(),
                state: DriveRequestState::Committed,
                response: Some(committed.clone()),
            })),
        ]);
        feature.observe(entry("2", "1")).unwrap();
        let output = feature
            .execute(
                "DriveWrite",
                &json!({"entry":entry("2", "1").entry,"content":"new"}).to_string(),
                &context,
                None,
            )
            .await
            .unwrap();
        assert_eq!(output.value["request_id"], id);
        assert_eq!(output.value["entry"]["metadata"]["last_mutation_id"], "2");
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0].method,
            crate::worker::WorkspaceRequestMethod::Post
        );
        assert_eq!(
            requests[1].method,
            crate::worker::WorkspaceRequestMethod::Get
        );
        assert!(requests[1].path.ends_with(&format!("/requests/{id}")));
        let body: Value = serde_json::from_str(requests[0].body.as_ref().unwrap()).unwrap();
        assert_eq!(body["request_id"], id);
        assert_eq!(body["mutation"]["expected_mutation_id"], "1");
    }
}

#[tokio::test]
async fn invalid_completion_and_unresolved_or_invalid_receipt_stay_unknown_after_one_query() {
    let context = ToolExecutionContext::new("write", "batch", 0);
    let client = Client::new(vec![]);
    let feature = feature(client.clone());
    let id = feature.request_id(&context);
    let committed = DriveMutationResponse {
        request_id: id.clone(),
        entry: Some(entry("2", "2")),
    };
    let mut receipts = vec![
        response(DriveRequestStatusResponse {
            request_id: id.clone(),
            state: DriveRequestState::Uncommitted,
            response: None,
        }),
        response(DriveRequestStatusResponse {
            request_id: id.clone(),
            state: DriveRequestState::Committed,
            response: None,
        }),
        response(DriveRequestStatusResponse {
            request_id: "another-request".into(),
            state: DriveRequestState::Committed,
            response: Some(committed),
        }),
        WorkspaceResponse {
            status: 200,
            body: "invalid-json".into(),
        },
    ];
    receipts.extend(invalid_completions(&id).into_iter().map(|completion| {
        response(DriveRequestStatusResponse {
            request_id: id.clone(),
            state: DriveRequestState::Committed,
            response: Some(completion),
        })
    }));
    for completion in invalid_completions(&id) {
        for receipt in &receipts {
            client.requests.lock().unwrap().clear();
            client
                .responses
                .lock()
                .unwrap()
                .extend([Ok(response(&completion)), Ok(receipt.clone())]);
            feature.observe(entry("2", "1")).unwrap();
            let result = feature
                .execute(
                    "DriveWrite",
                    &json!({"entry":entry("2", "1").entry,"content":"new"}).to_string(),
                    &context,
                    None,
                )
                .await;
            assert!(
                matches!(result, Err(DriveError::OutcomeUnknown(ref observed)) if observed == &id)
            );
            let requests = client.requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            assert_eq!(
                requests[0].method,
                crate::worker::WorkspaceRequestMethod::Post
            );
            assert_eq!(
                requests[1].method,
                crate::worker::WorkspaceRequestMethod::Get
            );
            assert!(requests[1].path.ends_with(&format!("/requests/{id}")));
        }
    }
}

#[tokio::test]
async fn observation_eviction_never_substitutes_another_entry() {
    let feature = feature(Client::new(Vec::new()));
    let first = entry("2", "1");
    feature.observe(first.clone()).unwrap();
    for id in 3..=258 {
        feature.observe(entry(&id.to_string(), "1")).unwrap();
    }
    assert!(feature.observed(&first.entry, None).is_err());
    let state = feature.observations.lock().unwrap();
    assert_eq!(state.entries.len(), MAX_OBSERVATIONS);
    assert_eq!(state.order.len(), MAX_OBSERVATIONS);
}

#[tokio::test]
async fn backend_error_details_are_not_reflected_and_conflict_is_not_retried() {
    let client = Client::new(vec![WorkspaceResponse {
        status: 409,
        body: serde_json::to_string(&DriveApiError {
            code: DriveApiErrorCode::Conflict,
            classification: DriveFailureClassification::NotCommitted,
            message: "/secret/provider/path".into(),
        })
        .unwrap(),
    }]);
    let feature = feature(client.clone());
    let old = entry("2", "1");
    feature.observe(old.clone()).unwrap();
    let result = execute(&feature, "DriveDelete", json!({"entry":old.entry}))
        .await
        .err()
        .unwrap();
    assert!(matches!(result, DriveError::Conflict));
    assert!(!result.to_string().contains("secret"));
    assert_eq!(client.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn drive_native_entry_is_directly_resolvable_but_never_indexable_and_binds_identity() {
    use crate::wip::WipSubtreeProvider;
    let entry = entry("9007199254740993", "3");
    let client = Client::new(vec![response(entry.clone())]);
    let feature = feature(client.clone());
    let mut mounts = crate::wip::WipMountRegistry::new();
    wip::mount_drive_wip(&mut mounts, &feature).unwrap();
    let provider = wip::DriveProvider::new(feature);
    let path = entry_path(&entry.entry);
    let publication = provider.publication(&path).await.unwrap().unwrap();
    assert_eq!(publication.projection.route, path);
    assert_eq!(
        publication.scope.as_ref().unwrap().r#ref,
        publication.projection.object.r#ref
    );
    assert!(publication.projection.object.validator.is_some());
    assert_ne!(publication.projection.object.validator, Some(b"3".to_vec()));
    assert!(provider.children("/drive").await.unwrap().is_empty());
    assert!(
        provider
            .publication("/drive/other/9007199254740993")
            .await
            .unwrap()
            .is_none()
    );
    let operations = &publication.projection.descriptor.operations;
    assert!(operations.iter().any(|op| op.name == "view_image"));
    assert!(
        !operations
            .iter()
            .flat_map(|op| &op.parameters)
            .any(|parameter| parameter.name == "expected_mutation_id")
    );
    assert_eq!(client.requests.lock().unwrap().len(), 2);
}

#[derive(Debug)]
struct ChunkClient {
    revoke_after_chunk: std::sync::atomic::AtomicBool,
    json: Arc<Client>,
    chunks: Mutex<VecDeque<crate::worker::WorkspaceBinaryResponse>>,
    requests: Mutex<Vec<crate::worker::WorkspaceBinaryRequest>>,
}
impl WorkspaceClient for ChunkClient {
    fn workspace_id(&self) -> Option<&str> {
        Some("workspace")
    }
    fn kind(&self) -> &str {
        "drive-chunk-test"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn execute(
        &self,
        request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        self.json.execute(request)
    }
    fn execute_binary(
        &self,
        request: crate::worker::WorkspaceBinaryRequest,
    ) -> Result<crate::worker::WorkspaceBinaryResponse, WorkspaceClientError> {
        self.requests.lock().unwrap().push(request);
        if self
            .revoke_after_chunk
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            self.json
                .auth_denied
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(self
            .chunks
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected binary read"))
    }
}
fn chunk_client(
    metadata: DriveEntry,
    chunks: Vec<crate::worker::WorkspaceBinaryResponse>,
) -> Arc<ChunkClient> {
    Arc::new(ChunkClient {
        revoke_after_chunk: std::sync::atomic::AtomicBool::new(false),
        json: Client::new(vec![response(metadata)]),
        chunks: Mutex::new(chunks.into()),
        requests: Mutex::new(Vec::new()),
    })
}
fn chunk(bytes: Vec<u8>) -> crate::worker::WorkspaceBinaryResponse {
    crate::worker::WorkspaceBinaryResponse {
        status: 200,
        body: bytes,
    }
}
#[tokio::test]
async fn multi_chunk_image_uses_one_last_mutation_id_and_never_exposes_binary_in_text_content() {
    let mut metadata = entry("2", "9007199254740993");
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.resize(DRIVE_CHUNK_MAX_BYTES as usize + 13, 42);
    metadata.size = Some(bytes.len() as u32);
    let client = chunk_client(
        metadata.clone(),
        vec![
            chunk(bytes[..DRIVE_CHUNK_MAX_BYTES as usize].to_vec()),
            chunk(bytes[DRIVE_CHUNK_MAX_BYTES as usize..].to_vec()),
        ],
    );
    let feature = DriveFeature::new(client.clone(), Arc::new(WorkdirSessionRouter::new()));
    let output = execute(&feature, "DriveViewImage", json!({"entry":metadata.entry}))
        .await
        .unwrap();
    let Attachment::Image(image) = &output.attachments[0];
    assert_eq!(image.data(), bytes);
    assert!(output.value.to_string().len() < 2048);
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    for request in &*requests {
        assert!(
            request
                .path
                .contains("expected_mutation_id=9007199254740993")
        );
        assert_eq!(request.max_response_bytes, DRIVE_CHUNK_MAX_BYTES as usize);
    }
    assert!(requests[0].path.split('&').any(|pair| pair == "offset=0"));
    assert!(
        requests[1]
            .path
            .split('&')
            .any(|pair| pair == "offset=65536")
    );
}
#[tokio::test]
async fn expired_last_mutation_id_or_partial_chunk_aborts_image_without_rebinding_or_retrying() {
    for next in [
        crate::worker::WorkspaceBinaryResponse {
            status: 409,
            body: serde_json::to_vec(&DriveApiError::new(DriveApiErrorCode::Conflict)).unwrap(),
        },
        chunk(vec![1]), // second range requires 13 bytes, not a successful prefix
    ] {
        let mut metadata = entry("2", "3");
        metadata.size = Some(DRIVE_CHUNK_MAX_BYTES + 13);
        let mut first = vec![0; DRIVE_CHUNK_MAX_BYTES as usize];
        first[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        let client = chunk_client(metadata.clone(), vec![chunk(first), next]);
        let feature = DriveFeature::new(client.clone(), Arc::new(WorkdirSessionRouter::new()));
        assert!(
            execute(&feature, "DriveViewImage", json!({"entry":metadata.entry}))
                .await
                .is_err()
        );
        let requests = client.requests.lock().unwrap();
        assert_eq!(
            requests.len(),
            2,
            "must not retry an expired/truncated image"
        );
        assert!(
            requests
                .iter()
                .all(|request| request.path.contains("expected_mutation_id=3&"))
        );
        assert_eq!(
            client.json.requests.lock().unwrap().len(),
            1,
            "must not fetch a new last_mutation_id"
        );
    }
}
#[tokio::test]
async fn oversized_images_and_unknown_arguments_fail_before_binary_transfer() {
    let mut metadata = entry("2", "1");
    metadata.size = Some(tools::view_image::MAX_IMAGE_BYTES as u32 + 1);
    let client = chunk_client(metadata.clone(), vec![]);
    let feature = DriveFeature::new(client.clone(), Arc::new(WorkdirSessionRouter::new()));
    assert!(matches!(
        execute(&feature, "DriveViewImage", json!({"entry":metadata.entry})).await,
        Err(DriveError::Limit)
    ));
    assert!(client.requests.lock().unwrap().is_empty());
    assert!(
        execute(
            &feature,
            "DriveRead",
            json!({"entry":metadata.entry,"expected_mutation_id":"1"})
        )
        .await
        .is_err()
    );
    assert_eq!(client.json.requests.lock().unwrap().len(), 1);
}

fn native_context() -> crate::wip::WipCallContext {
    crate::wip::WipCallContext {
        result_namespaces: Default::default(),
        execution: ToolExecutionContext::direct(),
        security_context: "drive-test".into(),
    }
}
fn native_args(value: Value) -> std::collections::BTreeMap<String, wip_protocol::Value> {
    let wip_protocol::Value::Record(args) = crate::wip::json_to_wip(&value).unwrap() else {
        panic!("named arguments required")
    };
    args
}

#[tokio::test]
async fn drive_common_text_reads_use_bounded_complete_source_and_literal_window_search() {
    use crate::file_operation::Contract;
    use crate::wip::WipSubtreeProvider;
    let text = "α\r\nneedle.*\nNeedle.*\nlast\n";
    let mut entry = entry("2", "3");
    entry.size = Some(text.len() as u32);
    let read = DriveReadTextResponse {
        entry: entry.clone(),
        text: text.into(),
        truncated: false,
    };
    let client = Client::new(vec![
        response(entry.clone()),
        response(&read),
        response(&read),
    ]);
    let feature = feature(client.clone());
    let publication = wip::DriveProvider::new(feature)
        .publication(&entry_path(&entry.entry))
        .await
        .unwrap()
        .unwrap();
    let mut projection = publication.projection;
    assert!(
        projection
            .object
            .interfaces
            .contains(&Contract::ReadLines.reference())
    );
    assert!(
        projection
            .object
            .interfaces
            .contains(&Contract::SearchText.reference())
    );
    assert!(
        !projection
            .object
            .interfaces
            .contains(&Contract::Write.reference())
    );
    assert!(
        !projection
            .object
            .interfaces
            .contains(&Contract::Edit.reference())
    );
    assert!(
        projection
            .descriptor
            .operations
            .iter()
            .any(|op| op.name == "read" && op.parameters.iter().any(|p| p.name == "max_bytes"))
    );
    let original_handler = projection.handler.clone();
    assert!(crate::file_operation::select(
        &mut projection,
        &Contract::ReadLines.reference()
    ));
    let read = projection
        .handler
        .call(
            "read",
            &native_args(json!({"offset":1,"limit":2})),
            native_context(),
        )
        .await
        .unwrap_or_else(|_| panic!("common read failed"));
    let value = crate::wip::wip_to_json(&read.value).unwrap();
    assert_eq!(value["next_offset"], 3);
    assert_eq!(value["truncated"], true);
    assert!(value["content"].as_str().unwrap().contains("needle.*"));
    // The common selector wraps a provider handler once; reuse the original
    // publication for the independent SearchText contract.
    projection.handler = original_handler;
    let request = client.requests.lock().unwrap().last().unwrap().clone();
    assert!(request.path.contains("max_bytes=65536"));
    assert!(crate::file_operation::select(
        &mut projection,
        &Contract::SearchText.reference()
    ));
    let search = projection
        .handler
        .call(
            "search",
            &native_args(json!({"pattern":"needle.*","limit":3})),
            native_context(),
        )
        .await
        .unwrap_or_else(|_| panic!("common search failed"));
    let value = crate::wip::wip_to_json(&search.value).unwrap();
    assert_eq!(value["matches"], json!([{"line":2,"text":"needle.*"}]));
    assert_eq!(value["next_offset"], 3);
    assert_eq!(value["truncated"], true);
}

fn read_response(entry: &DriveEntry, text: &str) -> DriveReadTextResponse {
    DriveReadTextResponse {
        entry: entry.clone(),
        text: text.into(),
        truncated: false,
    }
}

#[tokio::test]
async fn drive_common_text_refuses_truncated_mismatched_stale_and_denied_reads() {
    use crate::wip::WipSubtreeProvider;
    use wip_protocol::ProtocolErrorCode;
    for (case, expected) in [
        ("truncated", ProtocolErrorCode::ResourceLimitExceeded),
        ("size", ProtocolErrorCode::ResourceLimitExceeded),
        ("stale", ProtocolErrorCode::ValidatorMismatch),
        ("denied", ProtocolErrorCode::PermissionDenied),
    ] {
        let entry = entry("2", "3");
        let mut read = read_response(&entry, "hello");
        match case {
            "truncated" => read.truncated = true,
            "size" => read.text = "hell".into(),
            "stale" => read.entry.last_mutation_id = "4".into(),
            _ => {}
        }
        let read = if case == "denied" {
            WorkspaceResponse {
                status: 403,
                body: "denied".into(),
            }
        } else {
            response(read)
        };
        let client = Client::new(vec![response(entry.clone()), read]);
        let projection = wip::DriveProvider::new(feature(client.clone()))
            .publication(&entry_path(&entry.entry))
            .await
            .unwrap()
            .unwrap()
            .projection;
        let result = projection
            .handler
            .call("read_lines", &native_args(json!({})), native_context())
            .await;
        assert!(
            matches!(result, Err(crate::wip::WipOperationError::Protocol(ref error)) if error.code == expected),
            "{case}"
        );
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 3, "{case}: no retries");
        assert!(
            requests
                .iter()
                .all(|request| request.method == crate::worker::WorkspaceRequestMethod::Get)
        );
    }
}

#[tokio::test]
async fn drive_common_text_rejects_oversized_source_before_read_and_remount_changes_object_validator()
 {
    use crate::wip::WipSubtreeProvider;
    let mut entry = entry("2", "3");
    entry.size = Some(server_api::DRIVE_CHUNK_MAX_BYTES + 1);
    let client = Client::new(vec![response(entry.clone()), response(entry.clone())]);
    let first = feature(client.clone());
    let second = first.clone();
    let path = entry_path(&entry.entry);
    let first = wip::DriveProvider::new(first)
        .publication(&path)
        .await
        .unwrap()
        .unwrap()
        .projection;
    let second = wip::DriveProvider::new(second)
        .publication(&path)
        .await
        .unwrap()
        .unwrap()
        .projection;
    assert_ne!(first.object.validator, second.object.validator);
    assert_eq!(first.object.interfaces, second.object.interfaces);
    let result = first
        .handler
        .call("read_lines", &native_args(json!({})), native_context())
        .await;
    assert!(
        matches!(result, Err(crate::wip::WipOperationError::Protocol(error)) if error.code == wip_protocol::ProtocolErrorCode::ResourceLimitExceeded)
    );
    assert_eq!(client.requests.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn drive_common_text_reads_keep_tool_permission_denial_before_backend_read() {
    use crate::wip::WipSubtreeProvider;
    let entry = entry("2", "3");
    let client = Client::new(vec![response(entry.clone())]);
    let feature = feature(client.clone()).with_permissions(Some(manifest::ToolPermissionConfig {
        default_action: manifest::ToolPermissionAction::Deny,
        rules: Vec::new(),
    }));
    let projection = wip::DriveProvider::new(feature)
        .publication(&entry_path(&entry.entry))
        .await
        .unwrap()
        .unwrap()
        .projection;
    let result = projection
        .handler
        .call("read_lines", &native_args(json!({})), native_context())
        .await;
    assert!(
        matches!(result, Err(crate::wip::WipOperationError::Protocol(error)) if error.code == wip_protocol::ProtocolErrorCode::PermissionDenied)
    );
    assert_eq!(client.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn drive_read_only_capability_omits_file_and_folder_mutation_contracts() {
    use crate::file_operation::Contract;
    use crate::wip::WipSubtreeProvider;
    for (kind, root) in [
        (DriveEntryKind::File, false),
        (DriveEntryKind::Folder, false),
        (DriveEntryKind::Folder, true),
    ] {
        let mut entry = entry("2", "3");
        entry.kind = kind;
        if root {
            entry.parent = None;
        }
        let path = if root {
            "/drive".into()
        } else {
            entry_path(&entry.entry)
        };
        let client = Client::new(vec![response(entry.clone())]);
        let projection = wip::DriveProvider::new(feature(client.clone()))
            .publication(&path)
            .await
            .unwrap()
            .unwrap()
            .projection;
        assert!(
            !projection
                .object
                .interfaces
                .contains(&Contract::Edit.reference())
        );
        assert!(
            !projection
                .object
                .interfaces
                .contains(&Contract::Write.reference())
        );
        assert!(!projection.descriptor.operations.iter().any(|op| matches!(
            op.name.as_str(),
            "write"
                | "edit"
                | "delete"
                | "relocate"
                | "create_folder"
                | "create_text"
                | "save_workdir"
        )));
        let result = projection
            .handler
            .call(
                "write",
                &native_args(json!({"content":"denied"})),
                native_context(),
            )
            .await;
        assert!(
            matches!(result, Err(crate::wip::WipOperationError::Protocol(error)) if error.code == wip_protocol::ProtocolErrorCode::OperationNotFound)
        );
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].path, "/api/w/workspace/drive/capabilities");
        assert_eq!(
            requests[1].method,
            crate::worker::WorkspaceRequestMethod::Get
        );
        assert!(requests[1].body.is_none());
    }
}

#[tokio::test]
async fn drive_capability_failure_never_publishes_an_assumed_write_ceiling() {
    use crate::wip::WipSubtreeProvider;
    for (status, body) in [
        (403, "denied"),
        (500, "unavailable"),
        (200, "{}"),
        (200, "{\"write\":true,\"grant_id\":\"spoof\"}"),
    ] {
        let entry = entry("2", "3");
        let client = Client::new(vec![response(entry.clone())]);
        *client.capabilities.lock().unwrap() = WorkspaceResponse {
            status,
            body: body.into(),
        };
        let result = wip::DriveProvider::new(feature(client.clone()))
            .publication(&entry_path(&entry.entry))
            .await;
        assert!(!matches!(result, Ok(Some(_))), "{status}: {body}");
        assert_eq!(client.requests.lock().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn drive_common_mutation_publication_intersects_tool_permission_ceiling() {
    use crate::file_operation::Contract;
    use crate::wip::WipSubtreeProvider;
    let entry = entry("2", "3");
    let client = Client::new(vec![response(entry.clone())]);
    *client.capabilities.lock().unwrap() = response(DriveCapabilities { write: true });
    let feature = feature(client).with_permissions(Some(manifest::ToolPermissionConfig {
        default_action: manifest::ToolPermissionAction::Allow,
        rules: vec![manifest::ToolPermissionRule {
            tool: "DriveWrite".into(),
            pattern: "*".into(),
            action: manifest::ToolPermissionAction::Deny,
        }],
    }));
    let projection = wip::DriveProvider::new(feature)
        .publication(&entry_path(&entry.entry))
        .await
        .unwrap()
        .unwrap()
        .projection;
    assert!(
        !projection
            .object
            .interfaces
            .contains(&Contract::Write.reference())
    );
    assert!(
        projection
            .object
            .interfaces
            .contains(&Contract::Edit.reference())
    );
    assert!(
        !projection
            .descriptor
            .operations
            .iter()
            .any(|op| matches!(op.name.as_str(), "write" | "edit"))
    );
}

/// Narrow transport fixture for native mutation chaining. The real provider,
/// Client/Host, permission checks and Backend CAS adapter remain in the path;
/// only canonical Drive persistence is replaced by an entry and text value.
#[derive(Debug)]
struct NativeMutations(Mutex<(DriveEntry, String, Vec<DriveMutationRequest>)>);
impl WorkspaceClient for NativeMutations {
    fn workspace_id(&self) -> Option<&str> {
        Some("workspace")
    }
    fn kind(&self) -> &str {
        "drive-native-mutations"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn execute(
        &self,
        request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        if request.path.ends_with("/authorization/check") {
            let input: server_api::authorization::OperationAuthorizationRequest =
                serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
            return Ok(WorkspaceResponse {
                status: 200,
                body: serde_json::json!({"scope": input.resource}).to_string(),
            });
        }
        let mut state = self.0.lock().unwrap();
        let suffix = request
            .path
            .strip_prefix("/api/w/workspace/drive/")
            .unwrap();
        Ok(if suffix == "capabilities" {
            response(DriveCapabilities { write: true })
        } else if suffix.starts_with("metadata?") {
            response(&state.0)
        } else if suffix.starts_with("read-text?") {
            response(read_response(&state.0, &state.1))
        } else if suffix == "mutate" {
            let request: DriveMutationRequest =
                serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
            let DriveMutation::UpdateText {
                id,
                expected_mutation_id,
                text,
                ..
            } = &request.mutation
            else {
                panic!("unexpected mutation")
            };
            assert_eq!(id, &state.0.entry);
            assert_eq!(expected_mutation_id, &state.0.last_mutation_id);
            state.1 = text.clone();
            state.0.size = Some(text.len() as u32);
            state.0.last_mutation_id = request.request_id.clone();
            let output = response(DriveMutationResponse {
                request_id: request.request_id.clone(),
                entry: Some(state.0.clone()),
            });
            state.2.push(request);
            output
        } else {
            panic!("unexpected request: {}", request.path)
        })
    }
}

#[tokio::test]
async fn drive_common_edit_then_write_reuses_exact_post_operation_object_validator() {
    use crate::file_operation::Contract;
    use crate::wip::{WipMountRegistry, WipRuntime};
    let entry = entry("2", "3");
    let path = entry_path(&entry.entry);
    let client = Arc::new(NativeMutations(Mutex::new((
        entry,
        "hello".into(),
        Vec::new(),
    ))));
    let feature = DriveFeature::new(client.clone(), Arc::new(WorkdirSessionRouter::new()));
    let mut mounts = WipMountRegistry::new();
    wip::mount_drive_wip(&mut mounts, &feature).unwrap();
    let runtime = WipRuntime::from_mounts(mounts, "drive-test".into()).unwrap();
    runtime
        .prepare_call_observations(path.clone(), true)
        .await
        .unwrap();
    let edit = runtime
        .call(
            path.clone(),
            Contract::Edit.reference(),
            "edit".into(),
            json!({"old_string":"hello","new_string":"updated"}),
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
    let edit: Value = serde_json::from_str(edit.content.as_deref().unwrap()).unwrap();
    assert!(edit["metadata"]["entry"]["metadata"]["last_mutation_id"].is_string());
    assert_eq!(client.0.lock().unwrap().1, "updated");
    // No intervening Inspect/prepare_call_observations: success depends on the
    // exact nonce-bound post-commit validator returned by the first operation.
    let write = runtime
        .call(
            path,
            Contract::Write.reference(),
            "write".into(),
            json!({"content":"final"}),
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
    let write: Value = serde_json::from_str(write.content.as_deref().unwrap()).unwrap();
    assert!(write["metadata"]["entry"]["metadata"]["last_mutation_id"].is_string());
    let state = client.0.lock().unwrap();
    assert_eq!(state.1, "final");
    assert_eq!(state.2.len(), 2);
}

#[tokio::test]
async fn drive_argument_dependent_write_policy_is_rechecked_and_ask_is_not_published() {
    use crate::file_operation::Contract;
    use crate::wip::WipSubtreeProvider;
    for (default_action, pattern, action, published, code) in [
        (
            manifest::ToolPermissionAction::Deny,
            "*permitted*",
            manifest::ToolPermissionAction::Allow,
            true,
            wip_protocol::ProtocolErrorCode::PermissionDenied,
        ),
        (
            manifest::ToolPermissionAction::Allow,
            "*",
            manifest::ToolPermissionAction::Ask,
            false,
            wip_protocol::ProtocolErrorCode::OperationNotFound,
        ),
    ] {
        let entry = entry("2", "3");
        let client = Client::new(vec![response(entry.clone())]);
        *client.capabilities.lock().unwrap() = response(DriveCapabilities { write: true });
        let feature =
            feature(client.clone()).with_permissions(Some(manifest::ToolPermissionConfig {
                default_action,
                rules: vec![manifest::ToolPermissionRule {
                    tool: "DriveWrite".into(),
                    pattern: pattern.into(),
                    action,
                }],
            }));
        let projection = wip::DriveProvider::new(feature)
            .publication(&entry_path(&entry.entry))
            .await
            .unwrap()
            .unwrap()
            .projection;
        assert_eq!(
            projection
                .object
                .interfaces
                .contains(&Contract::Write.reference()),
            published
        );
        let result = projection
            .handler
            .call(
                "write",
                &native_args(json!({"content":"denied"})),
                native_context(),
            )
            .await;
        assert!(
            matches!(result, Err(crate::wip::WipOperationError::Protocol(error)) if error.code == code)
        );
        assert_eq!(client.requests.lock().unwrap().len(), 2);
    }
}

// A single retained Object observation models a restored Client cache. All later
// publication and execution use the real new-mount provider; the new Host supplies
// the common contract's root scope independently, without replaying its old ref.
struct RetainedObjectOnce {
    current: wip::DriveProvider,
    retained: Mutex<Option<wip_protocol::Object>>,
}
#[async_trait]
impl crate::wip::WipSubtreeProvider for RetainedObjectOnce {
    async fn publication(
        &self,
        path: &str,
    ) -> Result<Option<crate::wip::WipPublication>, wip_protocol::ProtocolError> {
        let Some(mut publication) = self.current.publication(path).await? else {
            return Ok(None);
        };
        if let Some(retained) = self.retained.lock().unwrap().take() {
            publication.projection.object = retained;
            publication = crate::wip::WipPublication::self_scoped(publication.projection)?;
        }
        Ok(Some(publication))
    }
    async fn children(&self, _: &str) -> Result<Vec<String>, wip_protocol::ProtocolError> {
        Ok(Vec::new())
    }
}

#[tokio::test]
async fn drive_same_feature_remount_rejects_retained_object_with_fresh_common_scope_then_reobservation_succeeds()
 {
    use crate::file_operation::Contract;
    use crate::wip::{WipMountRegistry, WipRuntime, WipSubtreeMount, WipSubtreeProvider};
    let entry = entry("2", "3");
    let path = entry_path(&entry.entry);
    let client = Arc::new(NativeMutations(Mutex::new((
        entry,
        "hello".into(),
        Vec::new(),
    ))));
    let feature = DriveFeature::new(client.clone(), Arc::new(WorkdirSessionRouter::new()));
    // mount_drive_wip constructs exactly one of these providers per mount. Reuse
    // the same Feature, whose mutation request identity deliberately does not rotate.
    let first = wip::DriveProvider::new(feature.clone());
    let old = first.publication(&path).await.unwrap().unwrap().projection;
    let cloned = first
        .clone()
        .publication(&path)
        .await
        .unwrap()
        .unwrap()
        .projection;
    assert_eq!(old.object.validator, cloned.object.validator);
    let second = wip::DriveProvider::new(feature.clone());
    let current = second.publication(&path).await.unwrap().unwrap().projection;
    assert_ne!(old.object.validator, current.object.validator);
    let mut old_contract = old.clone();
    let mut current_contract = current.clone();
    assert!(crate::file_operation::select(
        &mut old_contract,
        &Contract::ReadLines.reference()
    ));
    assert!(crate::file_operation::select(
        &mut current_contract,
        &Contract::ReadLines.reference()
    ));
    assert_eq!(old_contract.interface, current_contract.interface);
    assert_eq!(old_contract.descriptor, current_contract.descriptor);
    assert_eq!(
        old_contract.interface_validator,
        current_contract.interface_validator
    );

    // Mount only the selected entry for this cache-restoration fixture. The real
    // Host still selects the root common contract and validates target membership,
    // scope_ref and Object/Interface validators before running the Drive handler.
    let mut mounts = WipMountRegistry::new();
    mounts.allocate_namespace("drive", "drive").unwrap();
    mounts.mount(current).unwrap();
    mounts
        .mount_subtree(WipSubtreeMount {
            root: path.clone(),
            provider: Arc::new(RetainedObjectOnce {
                current: second,
                retained: Mutex::new(Some(old.object)),
            }),
        })
        .unwrap();
    let runtime = WipRuntime::from_mounts(mounts, "drive-remount".into()).unwrap();
    runtime.inspect(path.clone(), true).await.unwrap();
    let interface_path = format!(
        "{}::{}",
        serde_json::to_string("/").unwrap(),
        serde_json::to_string(&Contract::ReadLines.reference().name).unwrap()
    );
    // This fetch obtains the *new* Host's root scope_ref and the unchanged common
    // Interface validator; only the retained target observation remains stale.
    runtime.inspect(interface_path, true).await.unwrap();
    let error = runtime
        .call(
            path.clone(),
            Contract::ReadLines.reference(),
            "read".into(),
            json!({}),
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("ValidatorMismatch"), "{error}");
    assert!(!error.contains("InterfaceMismatch"), "{error}");
    runtime.inspect(path.clone(), true).await.unwrap();
    let result = runtime
        .call(
            path,
            Contract::ReadLines.reference(),
            "read".into(),
            json!({}),
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
    let result: Value = serde_json::from_str(result.content.as_deref().unwrap()).unwrap();
    assert!(result["content"].as_str().unwrap().contains("hello"));
    assert!(client.0.lock().unwrap().2.is_empty());
}

// The real Tool/backend/client authorization connection is exercised; only Host transport is substituted.
#[tokio::test]
async fn denied_drive_operations_do_not_reach_provider_and_bind_actual_resources() {
    use server_api::authorization::PermissionResource;
    for operation in [
        "capabilities",
        "root",
        "metadata",
        "list",
        "search",
        "read_text",
        "create_folder",
        "create_text",
        "update_text",
        "relocate",
        "delete",
        "upload",
        "request_status",
    ] {
        let client = Client::new(vec![]);
        client
            .auth_denied
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let backend = DriveBackend::new(client.clone());
        let entry = entry("2", "old");
        let result = match operation {
            "capabilities" => backend.capabilities().await.map(|_| ()),
            "root" => backend.root().await.map(|_| ()),
            "metadata" => backend.metadata(&entry.entry).await.map(|_| ()),
            "list" => backend.list(&entry.entry, None, None).await.map(|_| ()),
            "search" => backend
                .search(serde_json::from_value(json!({"query":"note"})).unwrap())
                .await
                .map(|_| ()),
            "read_text" => backend.read(&entry.entry, 10).await.map(|_| ()),
            "request_status" => backend.status("request-1").await.map(|_| ()),
            "upload" => backend
                .upload(
                    DriveUploadQuery {
                        operation: DriveUploadOperation::Create,
                        request_id: "request-1".into(),
                        entry_workspace_id: "workspace".into(),
                        parent_id: Some("2".into()),
                        name: Some("new".into()),
                        id: None,
                        expected_mutation_id: None,
                        content_type: "text/plain".into(),
                        size: 0,
                        sha256: digest(b""),
                    },
                    vec![],
                )
                .await
                .map(|_| ()),
            _ => {
                let mutation = match operation {
                    "create_folder" => DriveMutation::CreateFolder {
                        parent: entry.entry,
                        name: "new".into(),
                    },
                    "create_text" => DriveMutation::CreateText {
                        parent: entry.entry,
                        name: "new".into(),
                        text: "new".into(),
                        content_type: "text/plain".into(),
                    },
                    "update_text" => DriveMutation::UpdateText {
                        id: entry.entry,
                        expected_mutation_id: "old".into(),
                        text: "new".into(),
                        content_type: "text/plain".into(),
                    },
                    "relocate" => DriveMutation::Relocate {
                        id: entry.entry,
                        expected_mutation_id: "old".into(),
                        parent: DriveEntryRef {
                            workspace_id: "workspace".into(),
                            node_id: "3".into(),
                        },
                        name: "new".into(),
                    },
                    "delete" => DriveMutation::Delete {
                        id: entry.entry,
                        expected_mutation_id: "old".into(),
                    },
                    _ => unreachable!(),
                };
                backend
                    .mutate(DriveMutationRequest {
                        request_id: "request-1".into(),
                        mutation,
                    })
                    .await
                    .map(|_| ())
            }
        };
        assert!(
            matches!(result, Err(DriveError::Authorization { denied: true, .. })),
            "{operation}: {result:?}"
        );
        assert!(client.requests.lock().unwrap().is_empty(), "{operation}");
        let auth = client.authorizations.lock().unwrap();
        assert_eq!(auth.len(), 1);
        assert_eq!(auth[0].operation, format!("drive.{operation}"));
        let expected = match operation {
            "capabilities" | "root" | "search" => PermissionResource::Workspace {
                workspace_id: "workspace".into(),
            },
            "request_status" => PermissionResource::DriveRequest {
                workspace_id: "workspace".into(),
                request_id: "request-1".into(),
            },
            _ => PermissionResource::DriveEntry {
                workspace_id: "workspace".into(),
                node_id: "2".into(),
            },
        };
        assert_eq!(auth[0].resource, expected);
    }
}

#[tokio::test]
async fn image_permission_revocation_blocks_next_chunk_and_returns_no_attachment() {
    let mut metadata = entry("2", "old");
    metadata.size = Some(DRIVE_CHUNK_MAX_BYTES + 1);
    let client = chunk_client(
        metadata.clone(),
        vec![chunk(vec![0; DRIVE_CHUNK_MAX_BYTES as usize])],
    );
    client
        .revoke_after_chunk
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let feature = DriveFeature::new(client.clone(), Arc::new(WorkdirSessionRouter::new()));
    let result = execute(&feature, "DriveViewImage", json!({"entry":metadata.entry})).await;
    assert!(matches!(
        result,
        Err(DriveError::Authorization { denied: true, .. })
    ));
    assert_eq!(client.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn drive_relocate_checks_destination_before_any_mutation() {
    use server_api::authorization::PermissionResource;
    let client = Client::new(vec![]);
    *client.denied_resource.lock().unwrap() = Some(PermissionResource::DriveEntry {
        workspace_id: "workspace".into(),
        node_id: "3".into(),
    });
    let backend = DriveBackend::new(client.clone());
    let result = backend
        .mutate(DriveMutationRequest {
            request_id: "request-1".into(),
            mutation: DriveMutation::Relocate {
                id: entry("2", "old").entry,
                expected_mutation_id: "old".into(),
                parent: entry("3", "old").entry,
                name: "moved".into(),
            },
        })
        .await;
    assert!(matches!(
        result,
        Err(DriveError::Authorization { denied: true, .. })
    ));
    assert!(client.requests.lock().unwrap().is_empty());
    let auth = client.authorizations.lock().unwrap();
    assert_eq!(auth.len(), 2);
    assert_eq!(
        auth[1].resource,
        PermissionResource::DriveEntry {
            workspace_id: "workspace".into(),
            node_id: "3".into()
        }
    );
}
