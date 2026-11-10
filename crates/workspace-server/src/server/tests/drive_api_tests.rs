//! Real isolated Server DB + FeatureStorage + LocalFileSystem HTTP boundaries.
use super::*;
use server_api::*;
use sha2::{Digest, Sha256};

fn base() -> String {
    format!("/api/w/{TEST_WORKSPACE_ID}/drive")
}
fn reference(id: &str) -> Value {
    json!({"workspace_id":TEST_WORKSPACE_ID,"node_id":id})
}
fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn upload_url(request: &str, root: &str, name: &str, media: &str, bytes: &[u8]) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("operation", "create")
        .append_pair("request_id", request)
        .append_pair("entry_workspace_id", TEST_WORKSPACE_ID)
        .append_pair("parent_id", root)
        .append_pair("name", name)
        .append_pair("content_type", media)
        .append_pair("size", &bytes.len().to_string())
        .append_pair("sha256", &digest(bytes))
        .finish();
    format!("{}/upload?{query}", base())
}
async fn response_json(response: Response, expected: StatusCode) -> Value {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    let body: Value =
        serde_json::from_slice(&bytes).unwrap_or_else(|_| json!(String::from_utf8_lossy(&bytes)));
    assert_eq!(status, expected, "{body}");
    body
}
async fn user_call(
    api: &WorkspaceApi,
    token: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: StatusCode,
) -> Value {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"));
    let bytes = if let Some(body) = body {
        builder = builder.header(CONTENT_TYPE, "application/json");
        serde_json::to_vec(&body).unwrap()
    } else {
        vec![]
    };
    response_json(
        build_router(api.clone())
            .oneshot(builder.body(Body::from(bytes)).unwrap())
            .await
            .unwrap(),
        expected,
    )
    .await
}
async fn root_id(api: &WorkspaceApi, token: &str) -> String {
    user_call(
        api,
        token,
        "GET",
        &format!("{}/root", base()),
        None,
        StatusCode::OK,
    )
    .await["entry"]["node_id"]
        .as_str()
        .unwrap()
        .to_string()
}
async fn mutate(
    api: &WorkspaceApi,
    token: &str,
    request: &str,
    mutation: Value,
    expected: StatusCode,
) -> Value {
    user_call(
        api,
        token,
        "POST",
        &format!("{}/mutate", base()),
        Some(json!({"request_id":request,"mutation":mutation})),
        expected,
    )
    .await
}

#[tokio::test]
async fn drive_http_receipts_cas_stable_url_and_workspace_refs_follow_storage_authority() {
    let temp = tempfile::tempdir().unwrap();
    let api = test_api(temp.path()).await;
    let token = seed_test_owner_api_token(&api, TEST_WORKSPACE_ID);
    let root = root_id(&api, &token).await;
    let create = json!({"operation":"create_text","parent":reference(&root),"name":"日本語.md","text":"# Shared\n","content_type":"text/markdown"});
    let first = mutate(&api, &token, "create", create.clone(), StatusCode::OK).await;
    let id = first["entry"]["entry"]["node_id"].as_str().unwrap();
    let rev = first["entry"]["last_mutation_id"].as_str().unwrap();
    assert_eq!(
        first,
        mutate(&api, &token, "create", create.clone(), StatusCode::OK).await,
        "same request must replay"
    );
    mutate(&api, &token, "same-name", create, StatusCode::CONFLICT).await;
    let status = user_call(
        &api,
        &token,
        "GET",
        &format!("{}/requests/create", base()),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(status["state"], "committed");
    assert_eq!(status["response"], first);
    let text = user_call(
        &api,
        &token,
        "GET",
        &format!(
            "{}/read-text?entry_workspace_id={TEST_WORKSPACE_ID}&id={id}&max_bytes=65536",
            base()
        ),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(text["text"], "# Shared\n");
    let wrong=mutate(&api,&token,"foreign",json!({"operation":"delete","id":{"workspace_id":"foreign","node_id":id},"expected_mutation_id":rev}),StatusCode::FORBIDDEN).await;
    assert_eq!(wrong["yoi_failure"]["details"]["code"], "denied");
    assert!(wrong["yoi_failure"]["details"].get("storage").is_none());
    assert!(!wrong.to_string().contains("foreign"));
    user_call(
        &api,
        &token,
        "GET",
        &format!("{}/metadata?entry_workspace_id=foreign&id={id}", base()),
        None,
        StatusCode::FORBIDDEN,
    )
    .await;
    mutate(
        &api,
        &token,
        "traversal",
        json!({"operation":"create_folder","parent":reference(&root),"name":"../escape"}),
        StatusCode::BAD_REQUEST,
    )
    .await;
    let updated=mutate(&api,&token,"update",json!({"operation":"update_text","id":reference(id),"expected_mutation_id":rev,"text":"new","content_type":"text/plain"}),StatusCode::OK).await;
    for operation in [
        json!({"operation":"update_text","id":reference(id),"expected_mutation_id":rev,"text":"lost","content_type":"text/plain"}),
        json!({"operation":"delete","id":reference(id),"expected_mutation_id":rev}),
        json!({"operation":"relocate","id":reference(id),"expected_mutation_id":rev,"parent":reference(&root),"name":"lost"}),
    ] {
        mutate(&api, &token, "stale", operation, StatusCode::CONFLICT).await;
    }
    user_call(&api,&token,"GET",&format!("{}/read-chunk?entry_workspace_id={TEST_WORKSPACE_ID}&id={id}&expected_mutation_id={rev}&offset=0&length=1",base()),None,StatusCode::CONFLICT).await;
    let renamed=mutate(&api,&token,"rename",json!({"operation":"relocate","id":reference(id),"expected_mutation_id":updated["entry"]["last_mutation_id"],"parent":reference(&root),"name":"renamed.md"}),StatusCode::OK).await;
    // URL is independent of name/parent; metadata lookup still resolves the same ID.
    assert_eq!(renamed["entry"]["entry"], first["entry"]["entry"]);
    let deleted=mutate(&api,&token,"delete",json!({"operation":"delete","id":reference(id),"expected_mutation_id":renamed["entry"]["last_mutation_id"]}),StatusCode::OK).await;
    assert!(deleted["entry"].is_null());
    user_call(
        &api,
        &token,
        "GET",
        &format!(
            "{}/metadata?entry_workspace_id={TEST_WORKSPACE_ID}&id={id}",
            base()
        ),
        None,
        StatusCode::NOT_FOUND,
    )
    .await;
    let recreated=mutate(&api,&token,"recreate",json!({"operation":"create_text","parent":reference(&root),"name":"renamed.md","text":"","content_type":"text/plain"}),StatusCode::OK).await;
    assert_ne!(recreated["entry"]["entry"]["node_id"], id);
}

#[tokio::test]
async fn drive_binary_upload_download_enforce_digest_size_and_safe_active_media_headers() {
    let temp = tempfile::tempdir().unwrap();
    let api = test_api(temp.path()).await;
    let token = seed_test_owner_api_token(&api, TEST_WORKSPACE_ID);
    let root = root_id(&api, &token).await;
    for (i, bytes, media) in [
        (0, b"<script>alert(1)</script>".as_slice(), "text/html"),
        (1, b"\x89PNG\r\n\xff".as_slice(), "image/png"),
        (2, b"".as_slice(), "application/octet-stream"),
    ] {
        let path = upload_url(
            &format!("upload-{i}"),
            &root,
            &format!("file-{i}"),
            media,
            bytes,
        );
        let response = build_router(api.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(path)
                    .header("authorization", format!("Bearer {token}"))
                    .header(CONTENT_TYPE, "application/octet-stream")
                    .body(Body::from(bytes))
                    .unwrap(),
            )
            .await
            .unwrap();
        let saved = response_json(response, StatusCode::OK).await;
        let id = saved["entry"]["entry"]["node_id"].as_str().unwrap();
        let path = format!(
            "{}/download?entry_workspace_id={TEST_WORKSPACE_ID}&id={id}",
            base()
        );
        let response = build_router(api.clone())
            .oneshot(
                Request::builder()
                    .uri(&path)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CONTENT_TYPE], media);
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert!(
            response.headers()["content-disposition"]
                .to_str()
                .unwrap()
                .starts_with("attachment;")
        );
        assert_eq!(response.headers()[CACHE_CONTROL], "private, no-store");
        assert!(
            response.headers()["content-security-policy"]
                .to_str()
                .unwrap()
                .contains("sandbox")
        );
        assert!(response.headers()["etag"].to_str().unwrap().contains(id));
        assert_eq!(
            to_bytes(response.into_body(), workspace_drive::MAX_FILE_BYTES)
                .await
                .unwrap()
                .as_ref(),
            bytes
        );
        let response = build_router(api.clone())
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        response_json(response, StatusCode::UNAUTHORIZED).await;
    }
    let path = upload_url("bad-digest", &root, "bad", "text/plain", b"aaa");
    let response = build_router(api.clone())
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(&path)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from("bbb"))
                .unwrap(),
        )
        .await
        .unwrap();
    response_json(response, StatusCode::BAD_REQUEST).await;
    let response = build_router(api.clone())
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from("a"))
                .unwrap(),
        )
        .await
        .unwrap();
    response_json(response, StatusCode::BAD_REQUEST).await;
    let listing = user_call(
        &api,
        &token,
        "GET",
        &format!(
            "{}/list?entry_workspace_id={TEST_WORKSPACE_ID}&id={root}",
            base()
        ),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        listing["entries"].as_array().unwrap().len(),
        3,
        "mismatch cannot publish"
    );
}

async fn workers(api: &WorkspaceApi) -> (WorkdirlessFixtureRuntime, Vec<RuntimeWorkerRef>) {
    register_test_runtime(api, WorkdirlessFixtureRuntime::RUNTIME_ID).await;
    let runtime = WorkdirlessFixtureRuntime::default();
    api.runtime.register_or_replace(runtime.clone());
    let mut workers = vec![];
    for name in ["Drive reader", "Drive writer"] {
        let Json(created) = create_workspace_worker_as_owner(
            State(api.clone()),
            HeaderMap::new(),
            Json(CreateWorkspaceWorkerRequest {
                runtime_id: WorkdirlessFixtureRuntime::RUNTIME_ID.into(),
                display_name: name.into(),
                singleton_key: None,
                profile: Some("builtin:coder".into()),
                ticket_assignment: None,
                initial_submit: vec![],
                workdir_attachments: vec![],
                feature_connections: Default::default(),
                control_operation_id: Some(uuid::Uuid::now_v7().to_string()),
                authorization: test_worker_operation_authorization(&[
                    (
                        "drive",
                        &[
                            "drive.capabilities",
                            "drive.root",
                            "drive.search",
                            "drive.metadata",
                            "drive.list",
                            "drive.read_text",
                            "drive.read_chunk",
                            "drive.create_folder",
                            "drive.create_text",
                            "drive.update_text",
                            "drive.delete",
                            "drive.relocate",
                            "drive.upload",
                            "drive.request_status",
                        ],
                    ),
                    ("filesystem", &["filesystem.read", "filesystem.write"]),
                ]),
            }),
        )
        .await
        .unwrap();
        workers.push(RuntimeWorkerRef::new(created.runtime_id, created.worker_id));
    }
    (runtime, workers)
}
async fn signed(
    api: &WorkspaceApi,
    identity: &RuntimeIdentityMaterial,
    worker: Option<&str>,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: StatusCode,
) -> Value {
    let bytes = body
        .map(|v| serde_json::to_vec(&v).unwrap())
        .unwrap_or_default();
    response_json(
        build_router(api.clone())
            .oneshot(runtime_source_request(
                identity, worker, method, path, bytes,
            ))
            .await
            .unwrap(),
        expected,
    )
    .await
}
#[tokio::test]
async fn drive_two_workers_and_non_owner_use_grants_without_workdir_or_profile_authority() {
    let temp = tempfile::tempdir().unwrap();
    let mut api = test_api(temp.path()).await;
    let owner = seed_test_owner_api_token(&api, TEST_WORKSPACE_ID);
    let member = seed_test_api_token(api.store.as_ref(), "member");
    let (_runtime, workers) = workers(&api).await;
    let identity = RuntimeIdentityMaterial::generate(&workers[0].runtime_id).unwrap();
    configure_runtime_request_auth(&mut api, &identity, &workers[0].runtime_id);
    let root = root_id(&api, &member).await;
    let get = format!("{}/root", base());
    signed(
        &api,
        &identity,
        Some(&workers[0].worker_id),
        "GET",
        &get,
        None,
        StatusCode::FORBIDDEN,
    )
    .await;
    user_call(&api,&member,"POST",&format!("{}/grants",base()),Some(json!({"runtime_id":workers[0].runtime_id,"worker_id":workers[0].worker_id,"access":"read_write"})),StatusCode::FORBIDDEN).await;
    let mut grants = vec![];
    for (worker, access) in workers.iter().zip(["read_only", "read_write"]) {
        grants.push(user_call(&api,&owner,"POST",&format!("{}/grants",base()),Some(json!({"runtime_id":worker.runtime_id,"worker_id":worker.worker_id,"access":access})),StatusCode::OK).await);
        assert!(
            api.store
                .list_worker_workdir_links(TEST_WORKSPACE_ID, worker)
                .unwrap()
                .is_empty()
        );
        signed(
            &api,
            &identity,
            Some(&worker.worker_id),
            "GET",
            &get,
            None,
            StatusCode::OK,
        )
        .await;
    }
    let mutation = json!({"request_id":"worker-write","mutation":{"operation":"create_text","parent":reference(&root),"name":"shared.md","text":"shared","content_type":"text/markdown"}});
    signed(
        &api,
        &identity,
        Some(&workers[0].worker_id),
        "POST",
        &format!("{}/mutate", base()),
        Some(mutation.clone()),
        StatusCode::FORBIDDEN,
    )
    .await;
    let saved = signed(
        &api,
        &identity,
        Some(&workers[1].worker_id),
        "POST",
        &format!("{}/mutate", base()),
        Some(mutation.clone()),
        StatusCode::OK,
    )
    .await;
    let id = saved["entry"]["entry"]["node_id"].as_str().unwrap();
    let text_path = format!(
        "{}/read-text?entry_workspace_id={TEST_WORKSPACE_ID}&id={id}",
        base()
    );
    assert_eq!(
        signed(
            &api,
            &identity,
            Some(&workers[0].worker_id),
            "GET",
            &text_path,
            None,
            StatusCode::OK
        )
        .await["text"],
        "shared"
    );
    assert_eq!(
        user_call(&api, &member, "GET", &text_path, None, StatusCode::OK).await["text"],
        "shared"
    );
    signed(
        &api,
        &identity,
        None,
        "GET",
        &get,
        None,
        StatusCode::FORBIDDEN,
    )
    .await;
    signed(
        &api,
        &identity,
        Some("not-registered"),
        "GET",
        &get,
        None,
        StatusCode::UNAUTHORIZED,
    )
    .await;
    user_call(
        &api,
        &owner,
        "DELETE",
        &format!(
            "{}/grants/{}",
            base(),
            grants[1]["grant_id"].as_str().unwrap()
        ),
        None,
        StatusCode::OK,
    )
    .await;
    signed(
        &api,
        &identity,
        Some(&workers[1].worker_id),
        "POST",
        &format!("{}/mutate", base()),
        Some(mutation),
        StatusCode::FORBIDDEN,
    )
    .await;
    signed(
        &api,
        &identity,
        Some(&workers[1].worker_id),
        "GET",
        &format!("{}/requests/worker-write", base()),
        None,
        StatusCode::FORBIDDEN,
    )
    .await;
}

fn public_drive_error(
    failure: client_support::ClientError<DriveHttpApiError>,
    expected_status: u16,
    expected_code: DriveApiErrorCode,
) -> DriveHttpApiError {
    let client_support::ClientError::Public { status, error } = failure else {
        panic!("expected decoded Drive HTTP error, got {failure:?}");
    };
    assert_eq!(status.as_u16(), expected_status);
    assert_eq!(error.code, expected_code);
    // Decoding result facts is not permission to encode the received Problem again.
    assert!(api_macros::HttpError::custom_response(&error).is_none());
    assert!(
        serde_json::to_value(&error)
            .unwrap()
            .get("yoi_failure")
            .is_none()
    );
    error
}

#[tokio::test]
async fn drive_http_client_preserves_limit_rejection_and_unknown_without_reexecution() {
    let temp = tempfile::tempdir().unwrap();
    let api = test_api(temp.path()).await;
    seed_test_owner_api_token(&api, "drive-errors");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = build_router(api.clone());
    let _server = AbortServer(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    let client = ServerApiClient::builder(&url)
        .unwrap()
        .authorizer(TestBearerAuthorizer("api-token-drive-errors"))
        .request_timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap();
    let workspace = TEST_WORKSPACE_ID.to_owned();
    let root = client.drive_root(workspace.clone()).await.unwrap();
    let error = public_drive_error(
        client
            .drive_upload(
                workspace.clone(),
                DriveUploadQuery {
                    operation: DriveUploadOperation::Create,
                    request_id: "limit-no-write".into(),
                    entry_workspace_id: workspace.clone(),
                    parent_id: Some(root.entry.node_id.clone()),
                    name: Some("oversized".into()),
                    id: None,
                    expected_mutation_id: None,
                    content_type: "text/plain".into(),
                    size: DRIVE_FILE_MAX_BYTES + 1,
                    sha256: digest(b""),
                },
                Vec::new().into(),
            )
            .await
            .unwrap_err(),
        413,
        DriveApiErrorCode::Limit,
    );
    let details = error.details().unwrap();
    assert_eq!(details.operation, DriveHttpOperation::Upload);
    assert_eq!(
        details.classification,
        DriveFailureClassification::NotCommitted
    );
    assert!(details.storage.is_none());
    let error = public_drive_error(
        client
            .drive_metadata(
                workspace.clone(),
                DriveEntryQuery {
                    entry_workspace_id: workspace.clone(),
                    id: "01".into(),
                },
            )
            .await
            .unwrap_err(),
        400,
        DriveApiErrorCode::Invalid,
    );
    assert_eq!(
        error.details().unwrap().operation,
        DriveHttpOperation::Request
    );
    assert!(error.details().unwrap().storage.is_none());

    // Inject only a storage fault. HTTP authentication and current transaction
    // authorization remain the production implementations, not this callback.
    let fixture = api.drive().await.unwrap();
    let fault = fixture.with_authorizer(|connection, _| {
        connection.execute_batch("CREATE TRIGGER fail_http_receipt BEFORE INSERT ON drive_receipts BEGIN SELECT RAISE(ABORT, 'private SQL /host/identity'); END;")?;
        Ok(())
    });
    tokio::task::spawn_blocking(move || fault.check_authorized(false))
        .await
        .unwrap()
        .unwrap();
    let error = public_drive_error(
        client
            .drive_mutate(
                workspace.clone(),
                DriveMutationRequest {
                    request_id: "unknown-no-retry".into(),
                    mutation: DriveMutation::CreateFolder {
                        parent: root.entry.clone(),
                        name: "candidate".into(),
                    },
                },
            )
            .await
            .unwrap_err(),
        503,
        DriveApiErrorCode::OutcomeUnknown,
    );
    let details = error.details().unwrap();
    assert_eq!(details.classification, DriveFailureClassification::Unknown);
    assert_eq!(details.operation, DriveHttpOperation::Mutate);
    let storage = details.storage.unwrap();
    assert_eq!(
        storage.operation,
        workspace_drive::failure::DriveFailureOperation::CreateFolder
    );
    assert_eq!(
        storage.reason,
        workspace_drive::failure::DriveFailureReason::MetadataFailure
    );
    assert!(!format!("{error:?}").contains("private SQL"));
    assert!(
        error.source_failure().is_none(),
        "remote diagnostics are not a local source"
    );
    let status = client
        .drive_request_status(workspace.clone(), "unknown-no-retry".into())
        .await
        .unwrap();
    assert_eq!(status.state, DriveRequestState::Uncommitted);
    let page = client
        .drive_list(
            workspace,
            DriveListQuery {
                entry_workspace_id: TEST_WORKSPACE_ID.into(),
                id: root.entry.node_id,
                limit: Some(10),
                after: None,
            },
        )
        .await
        .unwrap();
    assert!(
        page.entries.is_empty(),
        "the client must not reexecute an Unknown mutation"
    );
    let cleanup = fixture.with_authorizer(|connection, _| {
        connection.execute_batch("DROP TRIGGER fail_http_receipt")?;
        Ok(())
    });
    tokio::task::spawn_blocking(move || cleanup.check_authorized(false))
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn drive_rejection_normalizer_preserves_typed_problem_status_body_and_headers() {
    for (status, retain_extension) in [
        (StatusCode::BAD_REQUEST, true),
        (StatusCode::PAYLOAD_TOO_LARGE, true),
        (StatusCode::BAD_REQUEST, false),
        (StatusCode::PAYLOAD_TOO_LARGE, false),
    ] {
        let error = <DriveHttpApiError as api_macros::HttpRequestError>::from_request_rejection(
            status.as_u16(),
            "private rejection diagnostic".into(),
        );
        let encoded = api_macros::HttpError::custom_response(&error).unwrap();
        let expected = encoded.body.as_ref().to_vec();
        let app = Router::new()
            .route(
                "/",
                get(move || async move {
                    let error =
                        <DriveHttpApiError as api_macros::HttpRequestError>::from_request_rejection(
                            status.as_u16(),
                            String::new(),
                        );
                    let mut response = api_macros::axum::http_error_response(status, error);
                    if !retain_extension {
                        response.extensions_mut().clear();
                    }
                    response
                        .headers_mut()
                        .insert("x-drive-probe", HeaderValue::from_static("retained"));
                    response
                }),
            )
            .layer(middleware::from_fn(drive::normalize_rejections));
        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()[CONTENT_TYPE], "application/problem+json");
        assert_eq!(response.headers()["x-drive-probe"], "retained");
        assert_eq!(
            to_bytes(response.into_body(), 64 * 1024)
                .await
                .unwrap()
                .as_ref(),
            expected.as_slice()
        );
    }
}

#[tokio::test]
async fn drive_client_decoder_checks_actual_status_and_media_before_accepting_details() {
    let app = Router::new().route(
        "/api/w/{workspace_id}/drive/root",
        get(|AxumPath(case): AxumPath<String>| async move {
            let error = <DriveHttpApiError as api_macros::HttpRequestError>::from_request_rejection(
                400,
                String::new(),
            );
            let encoded = api_macros::HttpError::custom_response(&error).unwrap();
            let (status, media) = match case.as_str() {
                "wrong-status" => (StatusCode::PAYLOAD_TOO_LARGE, "application/problem+json"),
                "wrong-json" => (StatusCode::BAD_REQUEST, "application/json"),
                "wrong-media" => (StatusCode::BAD_REQUEST, "text/plain"),
                _ => (
                    StatusCode::BAD_REQUEST,
                    "application/problem+json; charset=utf-8",
                ),
            };
            (status, [(CONTENT_TYPE, media)], encoded.body.into_bytes()).into_response()
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let _server = AbortServer(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    let client = ServerApiClient::builder(&url)
        .unwrap()
        .request_timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap();
    for (case, actual_status) in [("wrong-status", 413), ("wrong-json", 400)] {
        let error = client.drive_root(case.into()).await.unwrap_err();
        assert!(matches!(error, client_support::ClientError::Failure(
            client_support::ClientFailure::ErrorResponseDecode { status }
        ) if status == actual_status));
    }
    let error = client.drive_root("wrong-media".into()).await.unwrap_err();
    assert!(matches!(
        error,
        client_support::ClientError::Failure(
            client_support::ClientFailure::UnsupportedErrorResponse { status: 400 }
        )
    ));
    let received = public_drive_error(
        client.drive_root("valid".into()).await.unwrap_err(),
        400,
        DriveApiErrorCode::Invalid,
    );
    assert!(received.details().is_some());
}

struct AbortServer(tokio::task::JoinHandle<()>);
impl Drop for AbortServer {
    fn drop(&mut self) {
        self.0.abort();
    }
}
#[tokio::test]
async fn drive_generated_client_roundtrips_binary_and_declared_headers_over_real_http() {
    let temp = tempfile::tempdir().unwrap();
    let api = test_api(temp.path()).await;
    seed_test_owner_api_token(&api, "drive-client");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = build_router(api);
    let _server = AbortServer(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    let client = ServerApiClient::builder(&url)
        .unwrap()
        .authorizer(TestBearerAuthorizer("api-token-drive-client"))
        .response_body_limit(workspace_drive::MAX_FILE_BYTES)
        .build()
        .unwrap();
    let workspace = TEST_WORKSPACE_ID.to_string();
    let root = client.drive_root(workspace.clone()).await.unwrap();
    for (request, bytes, media) in [
        ("markdown", b"# Client\n".as_slice(), "text/markdown"),
        ("image", b"\x89PNG\xff".as_slice(), "image/png"),
        ("empty", b"".as_slice(), "application/octet-stream"),
    ] {
        let saved = client
            .drive_upload(
                workspace.clone(),
                DriveUploadQuery {
                    operation: DriveUploadOperation::Create,
                    request_id: request.into(),
                    entry_workspace_id: workspace.clone(),
                    parent_id: Some(root.entry.node_id.clone()),
                    name: Some(request.into()),
                    id: None,
                    expected_mutation_id: None,
                    content_type: media.into(),
                    size: bytes.len() as u32,
                    sha256: digest(bytes),
                },
                bytes.to_vec().into(),
            )
            .await
            .unwrap();
        let saved = saved.entry.unwrap();
        let response = client
            .drive_download(
                workspace.clone(),
                DriveDownloadQuery {
                    entry_workspace_id: workspace.clone(),
                    id: saved.entry.node_id.clone(),
                    expected_mutation_id: Some(saved.last_mutation_id.clone()),
                },
            )
            .await
            .unwrap();
        let server_api_responses::DriveDownload::Status200 {
            body,
            header_content_disposition,
            header_cache_control,
            header_x_content_type_options,
            ..
        } = response;
        assert_eq!(body.as_ref(), bytes);
        assert!(header_content_disposition.starts_with("attachment;"));
        assert_eq!(header_cache_control, "private, no-store");
        assert_eq!(header_x_content_type_options, "nosniff");
        let chunk = client
            .drive_read_chunk(
                workspace.clone(),
                DriveReadChunkQuery {
                    entry_workspace_id: workspace.clone(),
                    id: saved.entry.node_id,
                    expected_mutation_id: saved.last_mutation_id,
                    offset: 0,
                    length: 65536,
                },
            )
            .await
            .unwrap();
        assert_eq!(chunk.as_ref(), bytes);
    }
}

#[tokio::test]
async fn drive_stream_abort_invalid_query_and_storage_loss_never_publish_or_leak_paths() {
    let temp = tempfile::tempdir().unwrap();
    let api = test_api(temp.path()).await;
    let token = seed_test_owner_api_token(&api, TEST_WORKSPACE_ID);
    let root = root_id(&api, &token).await;
    let chunks = futures::stream::iter([
        Ok(axum::body::Bytes::from_static(b"a")),
        Err(std::io::Error::other("private host path")),
    ]);
    let path = upload_url("interrupted", &root, "broken", "text/plain", b"aaa");
    let response = build_router(api.clone())
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from_stream(chunks))
                .unwrap(),
        )
        .await
        .unwrap();
    let rejected = response_json(response, StatusCode::BAD_REQUEST).await;
    assert!(!rejected.to_string().contains("private host"));
    let status = user_call(
        &api,
        &token,
        "GET",
        &format!("{}/requests/interrupted", base()),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(status["state"], "uncommitted");
    let over =
        upload_url("oversized", &root, "big", "text/plain", b"").replace("size=0", "size=16777217");
    let response = build_router(api.clone())
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(over)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response_json(response, StatusCode::PAYLOAD_TOO_LARGE).await["yoi_failure"]["details"]["code"],
        "limit"
    );
    let malformed = user_call(
        &api,
        &token,
        "GET",
        &format!(
            "{}/metadata?entry_workspace_id={TEST_WORKSPACE_ID}&id=01",
            base()
        ),
        None,
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(malformed["yoi_failure"]["details"]["code"], "invalid");
    mutate(&api,&token,"fake-actor",json!({"operation":"create_folder","parent":reference(&root),"name":"fake","actor":"owner"}),StatusCode::BAD_REQUEST).await;
    let saved=mutate(&api,&token,"real",json!({"operation":"create_text","parent":reference(&root),"name":"real","text":"persisted","content_type":"text/plain"}),StatusCode::OK).await;
    let blob_root = api.config.drive_blob_root().unwrap();
    // Delete only the fixture's immutable blobs to simulate a Host storage failure.
    for file in fs::read_dir(blob_root).unwrap() {
        fs::remove_file(file.unwrap().path()).unwrap();
    }
    let id = saved["entry"]["entry"]["node_id"].as_str().unwrap();
    let response = build_router(api.clone())
        .oneshot(
            Request::builder()
                .uri(format!(
                    "{}/read-text?entry_workspace_id={TEST_WORKSPACE_ID}&id={id}",
                    base()
                ))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let local = response
        .extensions()
        .get::<Arc<DriveHttpApiError>>()
        .expect("local typed error");
    assert!(matches!(
        local.source_failure().unwrap().source_error(),
        workspace_drive::Error::Blob(_)
    ));
    let error = response_json(response, StatusCode::SERVICE_UNAVAILABLE).await;
    assert_eq!(
        error["yoi_failure"]["details"]["code"],
        "storage_unavailable"
    );
    assert_eq!(error["yoi_failure"]["details"]["operation"], "read_text");
    assert_eq!(
        error["yoi_failure"]["details"]["classification"],
        "not_committed"
    );
    assert_eq!(
        error["yoi_failure"]["details"]["storage"]["reason"],
        "blob_failure"
    );
    assert!(!error.to_string().contains(temp.path().to_str().unwrap()));
    user_call(
        &api,
        &token,
        "GET",
        &format!(
            "{}/metadata?entry_workspace_id={TEST_WORKSPACE_ID}&id={id}",
            base()
        ),
        None,
        StatusCode::OK,
    )
    .await;
}

#[tokio::test]
async fn drive_verified_upload_revoked_during_reception_cannot_commit_or_replay() {
    let temp = tempfile::tempdir().unwrap();
    let mut api = test_api(temp.path()).await;
    let owner = seed_test_owner_api_token(&api, TEST_WORKSPACE_ID);
    let root = root_id(&api, &owner).await;
    let (_runtime, workers) = workers(&api).await;
    let identity = RuntimeIdentityMaterial::generate(&workers[0].runtime_id).unwrap();
    configure_runtime_request_auth(&mut api, &identity, &workers[0].runtime_id);
    let grant=user_call(&api,&owner,"POST",&format!("{}/grants",base()),Some(json!({"runtime_id":workers[0].runtime_id,"worker_id":workers[0].worker_id,"access":"read_write"})),StatusCode::OK).await;
    let store = api.store.clone();
    let grant_id = grant["grant_id"].as_str().unwrap().to_owned();
    let account = format!("account-{TEST_WORKSPACE_ID}");
    let bytes = b"publish only while granted";
    let path = upload_url(
        "revoked-in-flight",
        &root,
        "not-published",
        "text/plain",
        bytes,
    );
    let body = Body::from_stream(futures::stream::once(async move {
        // This frame is polled after the HTTP adapter's early authorization.
        store
            .revoke_workspace_drive_grant(TEST_WORKSPACE_ID, &grant_id, &account)
            .unwrap();
        Ok::<_, std::io::Error>(axum::body::Bytes::from_static(bytes))
    }));
    // Exercise the authorized HTTP boundary after the separately tested signed
    // ingress has populated its trusted source; this is not a client header bypass.
    let app = generated_workspace_contract_router(ServerApiContractService::Workspace(api.clone()))
        .layer(Extension(
            crate::worker_source::VerifiedRuntimeRequestSource {
                token_id_hash: None,
                runtime_id: workers[0].runtime_id.clone(),
                worker_id: Some(workers[0].worker_id.clone()),
                authentication_binding_id: "fixture-binding".into(),
            },
        ));
    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(path)
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response_json(response, StatusCode::FORBIDDEN).await["yoi_failure"]["details"]["code"],
        "denied"
    );
    let status = user_call(
        &api,
        &owner,
        "GET",
        &format!("{}/requests/revoked-in-flight", base()),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(status["state"], "uncommitted");
    let page = user_call(
        &api,
        &owner,
        "GET",
        &format!(
            "{}/list?entry_workspace_id={TEST_WORKSPACE_ID}&id={root}",
            base()
        ),
        None,
        StatusCode::OK,
    )
    .await;
    assert!(page["entries"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn drive_read_entrypoints_use_current_grants_and_cannot_fallback_to_browser_actor() {
    let temp = tempfile::tempdir().unwrap();
    let mut api = test_api(temp.path()).await;
    let owner = seed_test_owner_api_token(&api, TEST_WORKSPACE_ID);
    let root = root_id(&api, &owner).await;
    let saved=mutate(&api,&owner,"matrix-file",json!({"operation":"create_text","parent":reference(&root),"name":"matrix","text":"bytes","content_type":"text/plain"}),StatusCode::OK).await;
    let id = saved["entry"]["entry"]["node_id"].as_str().unwrap();
    let rev = saved["entry"]["last_mutation_id"].as_str().unwrap();
    let (_runtime, workers) = workers(&api).await;
    let identity = RuntimeIdentityMaterial::generate(&workers[0].runtime_id).unwrap();
    configure_runtime_request_auth(&mut api, &identity, &workers[0].runtime_id);
    let routes = [
        format!("{}/root", base()),
        format!("{}/capabilities", base()),
        format!(
            "{}/metadata?entry_workspace_id={TEST_WORKSPACE_ID}&id={id}",
            base()
        ),
        format!(
            "{}/list?entry_workspace_id={TEST_WORKSPACE_ID}&id={root}",
            base()
        ),
        format!("{}/search?query=matrix", base()),
        format!(
            "{}/read-text?entry_workspace_id={TEST_WORKSPACE_ID}&id={id}",
            base()
        ),
        format!(
            "{}/read-chunk?entry_workspace_id={TEST_WORKSPACE_ID}&id={id}&expected_mutation_id={rev}&offset=0&length=4",
            base()
        ),
        format!(
            "{}/download?entry_workspace_id={TEST_WORKSPACE_ID}&id={id}",
            base()
        ),
        format!("{}/requests/matrix-file", base()),
    ];
    for path in &routes {
        signed(
            &api,
            &identity,
            Some(&workers[0].worker_id),
            "GET",
            path,
            None,
            StatusCode::FORBIDDEN,
        )
        .await;
    }
    let grant=user_call(&api,&owner,"POST",&format!("{}/grants",base()),Some(json!({"runtime_id":workers[0].runtime_id,"worker_id":workers[0].worker_id,"access":"read_only"})),StatusCode::OK).await;
    let response = build_router(api.clone())
        .oneshot(runtime_source_request(
            &identity,
            Some(&workers[0].worker_id),
            "PUT",
            &upload_url("ro-upload", &root, "denied", "text/plain", b"x"),
            b"x".to_vec(),
        ))
        .await
        .unwrap();
    assert_eq!(
        response_json(response, StatusCode::FORBIDDEN).await["yoi_failure"]["details"]["code"],
        "denied"
    );
    user_call(
        &api,
        &owner,
        "DELETE",
        &format!("{}/grants/{}", base(), grant["grant_id"].as_str().unwrap()),
        None,
        StatusCode::OK,
    )
    .await;
    for path in &routes {
        signed(
            &api,
            &identity,
            Some(&workers[0].worker_id),
            "GET",
            path,
            None,
            StatusCode::FORBIDDEN,
        )
        .await;
    }
    let mut context = ServerRequestContext {
        actor: Some(test_owner_actor()),
        worker_source: Some(ServerWorkerSource {
            runtime_id: workers[0].runtime_id.clone(),
            worker_id: workers[0].worker_id.clone(),
        }),
        runtime_source: None,
        origin: None,
        transport_headers: vec![],
    };
    assert_eq!(
        drive::root(&api, &context, TEST_WORKSPACE_ID)
            .await
            .unwrap_err()
            .code,
        DriveApiErrorCode::Denied
    );
    context.runtime_source = Some(ServerRuntimeSource {
        runtime_id: workers[0].runtime_id.clone(),
        worker_id: Some(workers[1].worker_id.clone()),
        authentication_binding_id: "fixture-binding".into(),
    });
    assert_eq!(
        drive::root(&api, &context, TEST_WORKSPACE_ID)
            .await
            .unwrap_err()
            .code,
        DriveApiErrorCode::Denied
    );
}

#[tokio::test]
async fn drive_concurrent_http_cas_has_one_winner_and_name_search_pages_are_scoped() {
    let temp = tempfile::tempdir().unwrap();
    let api = test_api(temp.path()).await;
    let token = seed_test_owner_api_token(&api, TEST_WORKSPACE_ID);
    let root = root_id(&api, &token).await;
    let saved=mutate(&api,&token,"concurrent-file",json!({"operation":"create_text","parent":reference(&root),"name":"candidate","text":"find me","content_type":"text/plain"}),StatusCode::OK).await;
    let id = &saved["entry"]["entry"]["node_id"];
    let rev = &saved["entry"]["last_mutation_id"];
    let request = |identity: &str, text: &str| {
        Request::builder().method("POST").uri(format!("{}/mutate",base())).header("authorization",format!("Bearer {token}")).header(CONTENT_TYPE,"application/json").body(Body::from(json!({"request_id":identity,"mutation":{"operation":"update_text","id":reference(id.as_str().unwrap()),"expected_mutation_id":rev,"text":text,"content_type":"text/plain"}}).to_string())).unwrap()
    };
    let app = build_router(api.clone());
    let (a, b) = tokio::join!(
        app.clone().oneshot(request("a", "find a")),
        app.oneshot(request("b", "find b"))
    );
    let mut statuses = [a.unwrap().status().as_u16(), b.unwrap().status().as_u16()];
    statuses.sort();
    assert_eq!(statuses, [200, 409]);
    mutate(
        &api,
        &token,
        "folder",
        json!({"operation":"create_folder","parent":reference(&root),"name":"other"}),
        StatusCode::OK,
    )
    .await;
    let page = user_call(
        &api,
        &token,
        "GET",
        &format!(
            "{}/list?entry_workspace_id={TEST_WORKSPACE_ID}&id={root}&limit=1",
            base()
        ),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(page["entries"].as_array().unwrap().len(), 1);
    let cursor = page["next_after"].as_str().unwrap();
    user_call(
        &api,
        &token,
        "GET",
        &format!(
            "{}/list?entry_workspace_id={TEST_WORKSPACE_ID}&id={root}&after={}",
            base(),
            cursor.replace(TEST_WORKSPACE_ID, "foreign")
        ),
        None,
        StatusCode::FORBIDDEN,
    )
    .await;
    let names = user_call(
        &api,
        &token,
        "GET",
        &format!("{}/search?query=find", base()),
        None,
        StatusCode::OK,
    )
    .await;
    assert!(names["entries"].as_array().unwrap().is_empty());
    let text = user_call(
        &api,
        &token,
        "GET",
        &format!("{}/search?query=find&include_text=true", base()),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(text["entries"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn drive_download_stream_expires_instead_of_mixing_last_mutation_ids() {
    let temp = tempfile::tempdir().unwrap();
    let api = test_api(temp.path()).await;
    let token = seed_test_owner_api_token(&api, TEST_WORKSPACE_ID);
    let root = root_id(&api, &token).await;
    let bytes = vec![b'a'; 128 * 1024];
    let path = upload_url(
        "stream-generation",
        &root,
        "large",
        "application/octet-stream",
        &bytes,
    );
    let response = build_router(api.clone())
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(bytes))
                .unwrap(),
        )
        .await
        .unwrap();
    let saved = response_json(response, StatusCode::OK).await;
    let id = saved["entry"]["entry"]["node_id"].as_str().unwrap();
    let rev = saved["entry"]["last_mutation_id"].as_str().unwrap();
    let response = build_router(api.clone())
        .oneshot(
            Request::builder()
                .uri(format!(
                    "{}/download?entry_workspace_id={TEST_WORKSPACE_ID}&id={id}",
                    base()
                ))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    mutate(&api,&token,"stream-update",json!({"operation":"update_text","id":reference(id),"expected_mutation_id":rev,"text":"generation b","content_type":"text/plain"}),StatusCode::OK).await;
    let mut stream = response.into_body().into_data_stream();
    let first = stream.next().await.unwrap().unwrap();
    assert_eq!(first.as_ref(), vec![b'a'; 64 * 1024]);
    assert!(
        stream.next().await.unwrap().is_err(),
        "next chunk must expire, not deliver generation b"
    );
}

#[tokio::test]
async fn drive_capabilities_expose_only_authenticated_current_caller_ceiling() {
    let temp = tempfile::tempdir().unwrap();
    let mut api = test_api(temp.path()).await;
    let owner = seed_test_owner_api_token(&api, TEST_WORKSPACE_ID);
    let (_runtime, workers) = workers(&api).await;
    let identity = RuntimeIdentityMaterial::generate(&workers[0].runtime_id).unwrap();
    configure_runtime_request_auth(&mut api, &identity, &workers[0].runtime_id);
    let path = format!("{}/capabilities", base());
    let unsigned = build_router(api.clone())
        .oneshot(Request::builder().uri(&path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(matches!(
        unsigned.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ));
    signed(
        &api,
        &identity,
        None,
        "GET",
        &path,
        None,
        StatusCode::FORBIDDEN,
    )
    .await;
    let mut grants = vec![];
    for (worker, access, write) in [
        (&workers[0], "read_only", false),
        (&workers[1], "read_write", true),
    ] {
        grants.push(user_call(&api,&owner,"POST",&format!("{}/grants",base()),Some(json!({"runtime_id":worker.runtime_id,"worker_id":worker.worker_id,"access":access})),StatusCode::OK).await);
        assert_eq!(
            signed(
                &api,
                &identity,
                Some(&worker.worker_id),
                "GET",
                &path,
                None,
                StatusCode::OK
            )
            .await,
            json!({"write":write})
        );
    }
    // Altering a signed source cannot acquire the other Worker's larger ceiling.
    let mut forged =
        runtime_source_request(&identity, Some(&workers[0].worker_id), "GET", &path, vec![]);
    forged
        .headers_mut()
        .insert("x-yoi-worker-id", workers[1].worker_id.parse().unwrap());
    let response = build_router(api.clone()).oneshot(forged).await.unwrap();
    assert!(matches!(
        response.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ));
    let foreign = path.replace(&format!("/w/{TEST_WORKSPACE_ID}/"), "/w/other-workspace/");
    let response = build_router(api.clone())
        .oneshot(runtime_source_request(
            &identity,
            Some(&workers[1].worker_id),
            "GET",
            &foreign,
            vec![],
        ))
        .await
        .unwrap();
    assert!(!response.status().is_success());
    signed(
        &api,
        &identity,
        Some("nonexistent-worker"),
        "GET",
        &path,
        None,
        StatusCode::UNAUTHORIZED,
    )
    .await;
    for (worker, grant) in workers.iter().zip(&grants) {
        user_call(
            &api,
            &owner,
            "DELETE",
            &format!("{}/grants/{}", base(), grant["grant_id"].as_str().unwrap()),
            None,
            StatusCode::OK,
        )
        .await;
        signed(
            &api,
            &identity,
            Some(&worker.worker_id),
            "GET",
            &path,
            None,
            StatusCode::FORBIDDEN,
        )
        .await;
    }
}
