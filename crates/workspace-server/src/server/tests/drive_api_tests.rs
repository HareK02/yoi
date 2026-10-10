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
    let token = seed_test_api_token(api.store.as_ref(), TEST_WORKSPACE_ID);
    let root = root_id(&api, &token).await;
    let create = json!({"operation":"create_text","parent":reference(&root),"name":"日本語.md","text":"# Shared\n","content_type":"text/markdown"});
    let first = mutate(&api, &token, "create", create.clone(), StatusCode::OK).await;
    let id = first["entry"]["entry"]["node_id"].as_str().unwrap();
    let rev = first["entry"]["revision"].as_str().unwrap();
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
    let wrong=mutate(&api,&token,"foreign",json!({"operation":"delete","id":{"workspace_id":"foreign","node_id":id},"expected_revision":rev}),StatusCode::FORBIDDEN).await;
    assert_eq!(wrong["code"], "denied");
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
    let updated=mutate(&api,&token,"update",json!({"operation":"update_text","id":reference(id),"expected_revision":rev,"text":"new","content_type":"text/plain"}),StatusCode::OK).await;
    for operation in [
        json!({"operation":"update_text","id":reference(id),"expected_revision":rev,"text":"lost","content_type":"text/plain"}),
        json!({"operation":"delete","id":reference(id),"expected_revision":rev}),
        json!({"operation":"relocate","id":reference(id),"expected_revision":rev,"parent":reference(&root),"name":"lost"}),
    ] {
        mutate(&api, &token, "stale", operation, StatusCode::CONFLICT).await;
    }
    user_call(&api,&token,"GET",&format!("{}/read-chunk?entry_workspace_id={TEST_WORKSPACE_ID}&id={id}&expected_revision={rev}&offset=0&length=1",base()),None,StatusCode::CONFLICT).await;
    let renamed=mutate(&api,&token,"rename",json!({"operation":"relocate","id":reference(id),"expected_revision":updated["entry"]["revision"],"parent":reference(&root),"name":"renamed.md"}),StatusCode::OK).await;
    // URL is independent of name/parent; metadata lookup still resolves the same ID.
    assert_eq!(renamed["entry"]["entry"], first["entry"]["entry"]);
    let deleted=mutate(&api,&token,"delete",json!({"operation":"delete","id":reference(id),"expected_revision":renamed["entry"]["revision"]}),StatusCode::OK).await;
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
    let token = seed_test_api_token(api.store.as_ref(), TEST_WORKSPACE_ID);
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
        let Json(created) = create_workspace_worker(
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
                control_operation_id: None,
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
    let owner = seed_test_api_token(api.store.as_ref(), TEST_WORKSPACE_ID);
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
    seed_test_api_token(api.store.as_ref(), "drive-client");
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
                    expected_revision: None,
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
                    expected_revision: Some(saved.revision.clone()),
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
                    expected_revision: saved.revision,
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
    let token = seed_test_api_token(api.store.as_ref(), TEST_WORKSPACE_ID);
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
        response_json(response, StatusCode::PAYLOAD_TOO_LARGE).await["code"],
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
    assert_eq!(malformed["code"], "invalid");
    mutate(&api,&token,"fake-actor",json!({"operation":"create_folder","parent":reference(&root),"name":"fake","actor":"owner"}),StatusCode::BAD_REQUEST).await;
    let saved=mutate(&api,&token,"real",json!({"operation":"create_text","parent":reference(&root),"name":"real","text":"persisted","content_type":"text/plain"}),StatusCode::OK).await;
    let blob_root = api.config.drive_blob_root().unwrap();
    // Delete only the fixture's immutable blobs to simulate a Host storage failure.
    for file in fs::read_dir(blob_root).unwrap() {
        fs::remove_file(file.unwrap().path()).unwrap();
    }
    let id = saved["entry"]["entry"]["node_id"].as_str().unwrap();
    let error = user_call(
        &api,
        &token,
        "GET",
        &format!(
            "{}/read-text?entry_workspace_id={TEST_WORKSPACE_ID}&id={id}",
            base()
        ),
        None,
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
    assert_eq!(error["code"], "storage_unavailable");
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
    let owner = seed_test_api_token(api.store.as_ref(), TEST_WORKSPACE_ID);
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
        response_json(response, StatusCode::FORBIDDEN).await["code"],
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
    let owner = seed_test_api_token(api.store.as_ref(), TEST_WORKSPACE_ID);
    let root = root_id(&api, &owner).await;
    let saved=mutate(&api,&owner,"matrix-file",json!({"operation":"create_text","parent":reference(&root),"name":"matrix","text":"bytes","content_type":"text/plain"}),StatusCode::OK).await;
    let id = saved["entry"]["entry"]["node_id"].as_str().unwrap();
    let rev = saved["entry"]["revision"].as_str().unwrap();
    let (_runtime, workers) = workers(&api).await;
    let identity = RuntimeIdentityMaterial::generate(&workers[0].runtime_id).unwrap();
    configure_runtime_request_auth(&mut api, &identity, &workers[0].runtime_id);
    let routes = [
        format!("{}/root", base()),
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
            "{}/read-chunk?entry_workspace_id={TEST_WORKSPACE_ID}&id={id}&expected_revision={rev}&offset=0&length=4",
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
        response_json(response, StatusCode::FORBIDDEN).await["code"],
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
    let token = seed_test_api_token(api.store.as_ref(), TEST_WORKSPACE_ID);
    let root = root_id(&api, &token).await;
    let saved=mutate(&api,&token,"concurrent-file",json!({"operation":"create_text","parent":reference(&root),"name":"candidate","text":"find me","content_type":"text/plain"}),StatusCode::OK).await;
    let id = &saved["entry"]["entry"]["node_id"];
    let rev = &saved["entry"]["revision"];
    let request = |identity: &str, text: &str| {
        Request::builder().method("POST").uri(format!("{}/mutate",base())).header("authorization",format!("Bearer {token}")).header(CONTENT_TYPE,"application/json").body(Body::from(json!({"request_id":identity,"mutation":{"operation":"update_text","id":reference(id.as_str().unwrap()),"expected_revision":rev,"text":text,"content_type":"text/plain"}}).to_string())).unwrap()
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
async fn drive_download_stream_expires_instead_of_mixing_revisions() {
    let temp = tempfile::tempdir().unwrap();
    let api = test_api(temp.path()).await;
    let token = seed_test_api_token(api.store.as_ref(), TEST_WORKSPACE_ID);
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
    let rev = saved["entry"]["revision"].as_str().unwrap();
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
    mutate(&api,&token,"stream-update",json!({"operation":"update_text","id":reference(id),"expected_revision":rev,"text":"generation b","content_type":"text/plain"}),StatusCode::OK).await;
    let mut stream = response.into_body().into_data_stream();
    let first = stream.next().await.unwrap().unwrap();
    assert_eq!(first.as_ref(), vec![b'a'; 64 * 1024]);
    assert!(
        stream.next().await.unwrap().is_err(),
        "next chunk must expire, not deliver generation b"
    );
}
