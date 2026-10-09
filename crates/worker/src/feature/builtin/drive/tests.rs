use super::*;
use crate::worker::{WorkspaceClientError, WorkspaceRequest, WorkspaceResponse};
use serde::Serialize;
use std::collections::VecDeque;

#[derive(Debug)]
struct Client {
    responses: Mutex<VecDeque<Result<WorkspaceResponse, WorkspaceClientError>>>,
    requests: Mutex<Vec<WorkspaceRequest>>,
}
impl Client {
    fn new(responses: Vec<WorkspaceResponse>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into_iter().map(Ok).collect()),
            requests: Mutex::new(Vec::new()),
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
        self.requests.lock().unwrap().push(request);
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
fn entry(id: &str, revision: &str) -> DriveEntry {
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
        revision: revision.into(),
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
fn drive_feature_activation_never_creates_authority_and_schemas_hide_revisions() {
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
        assert!(!schema.contains("expected_revision"), "{name}");
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
async fn stale_edit_returns_conflict_without_upgrading_revision_or_dispatching_write() {
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
    assert_eq!(feature.observed(&old.entry, None).unwrap().revision, "1");
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
async fn successful_mutation_uses_observed_decimal_revision_and_requires_next_explicit_observation()
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
    assert_eq!(body["mutation"]["expected_revision"], "9007199254740993");
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
    invalid_entry.revision = "not-a-revision".into();
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
        assert_eq!(output.value["entry"]["metadata"]["revision"], "2");
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
        assert_eq!(body["mutation"]["expected_revision"], "1");
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
    let provider = wip::DriveProvider(feature);
    let path = entry_path(&entry.entry);
    let publication = provider.publication(&path).await.unwrap().unwrap();
    assert_eq!(publication.projection.route, path);
    assert_eq!(
        publication.scope.as_ref().unwrap().r#ref,
        publication.projection.object.r#ref
    );
    assert_eq!(publication.projection.object.validator, Some(b"3".to_vec()));
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
            .any(|parameter| parameter.name == "expected_revision")
    );
    assert_eq!(client.requests.lock().unwrap().len(), 1);
}

#[derive(Debug)]
struct ChunkClient {
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
async fn multi_chunk_image_uses_one_revision_and_never_exposes_binary_in_text_content() {
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
        assert!(request.path.contains("expected_revision=9007199254740993"));
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
async fn expired_revision_or_partial_chunk_aborts_image_without_rebinding_or_retrying() {
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
                .all(|request| request.path.contains("expected_revision=3&"))
        );
        assert_eq!(
            client.json.requests.lock().unwrap().len(),
            1,
            "must not fetch a new revision"
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
            json!({"entry":metadata.entry,"expected_revision":"1"})
        )
        .await
        .is_err()
    );
    assert_eq!(client.json.requests.lock().unwrap().len(), 1);
}
