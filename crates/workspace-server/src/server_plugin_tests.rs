use super::*;

// Generated routes -> real token resolver and Workspace admission -> real catalog
// SQLite/blob storage. The unrelated Worker executor is the existing test backend.
fn package_bytes(version: &str) -> Vec<u8> {
    use manifest::plugin::*;
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("plugin.toml"),
        include_str!("../../../resources/plugin/templates/rust-component-tool/plugin.toml")
            .replace("0.1.0", version),
    )
    .unwrap();
    fs::write(
        dir.path().join("plugin.component.wasm"),
        include_bytes!("../../plugin-pdk/tests/fixtures/tool-v2.component.wasm"),
    )
    .unwrap();
    let limits = PluginPackageLimits::default();
    pack_plugin_package(
        &read_plugin_directory(dir.path(), PluginSourceKind::Explicit, &limits).unwrap(),
        &limits,
    )
    .unwrap()
    .bytes
}

async fn request(
    app: Router,
    token: Option<&str>,
    method: &str,
    path: &str,
    body: Vec<u8>,
    binary: bool,
    expected: StatusCode,
) -> Value {
    let mut req = Request::builder().method(method).uri(path).header(
        CONTENT_TYPE,
        if binary {
            "application/octet-stream"
        } else {
            "application/json"
        },
    );
    if let Some(token) = token {
        req = req.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 32 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        status,
        expected,
        "{method} {path}: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).unwrap()
}
async fn json(app: Router, token: &str, path: &str, body: Value, expected: StatusCode) -> Value {
    request(
        app,
        Some(token),
        "POST",
        path,
        serde_json::to_vec(&body).unwrap(),
        false,
        expected,
    )
    .await
}
async fn get(app: Router, token: Option<&str>, path: &str, expected: StatusCode) -> Value {
    request(app, token, "GET", path, vec![], false, expected).await
}

#[tokio::test]
async fn authenticated_catalog_lifecycle_replay_cas_and_current_read_authority() {
    let dir = tempfile::tempdir().unwrap();
    let api = test_api(dir.path()).await;
    let owner = seed_test_workspace_owner_token(api.store.as_ref(), "plugin-owner").await;
    let reader = seed_test_api_token(api.store.as_ref(), "plugin-reader");
    let app = build_router(api.clone());
    let prefix = format!("/api/w/{TEST_WORKSPACE_ID}");
    let catalog = format!("{prefix}/plugins");
    let upload_path = format!("{prefix}/plugin-uploads");
    get(app.clone(), None, &catalog, StatusCode::UNAUTHORIZED).await;
    get(app.clone(), Some(&reader), &catalog, StatusCode::OK).await;
    request(
        app.clone(),
        Some(&reader),
        "POST",
        &upload_path,
        package_bytes("0.1.0"),
        true,
        StatusCode::FORBIDDEN,
    )
    .await;
    let upload = request(
        app.clone(),
        Some(&owner),
        "POST",
        &upload_path,
        package_bytes("0.1.0"),
        true,
        StatusCode::OK,
    )
    .await;
    assert_ne!(
        upload["artifact"]["package_digest"],
        upload["artifact"]["artifact_digest"]
    );
    let install = serde_json::json!({"upload_id":upload["upload_id"],"alias":"primary","operation_key":"install"});
    let original = json(
        app.clone(),
        &owner,
        &catalog,
        install.clone(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        json(
            app.clone(),
            &owner,
            &catalog,
            install.clone(),
            StatusCode::OK
        )
        .await,
        original
    );
    let mut changed = install.clone();
    changed["alias"] = "other".into();
    json(app.clone(), &owner, &catalog, changed, StatusCode::CONFLICT).await;
    let id = original["installation"]["installation_id"]
        .as_str()
        .unwrap();
    let detail_path = format!("{catalog}/{id}");
    let observed = get(app.clone(), Some(&reader), &detail_path, StatusCode::OK).await;
    assert_eq!(observed["last_operation"], original["operation_id"]);
    get(
        app.clone(),
        Some(&reader),
        &format!(
            "{prefix}/plugin-operations/{}",
            original["operation_id"].as_str().unwrap()
        ),
        StatusCode::OK,
    )
    .await;
    get(
        app.clone(),
        Some(&owner),
        &format!("/api/w/another/plugins/{id}"),
        StatusCode::NOT_FOUND,
    )
    .await;
    get(
        app.clone(),
        Some(&owner),
        &format!(
            "{prefix}/plugins/{}",
            upload["artifact"]["artifact_digest"].as_str().unwrap()
        ),
        StatusCode::NOT_FOUND,
    )
    .await;
    let next_upload = request(
        app.clone(),
        Some(&owner),
        "POST",
        &upload_path,
        package_bytes("0.2.0"),
        true,
        StatusCode::OK,
    )
    .await;
    let add = json(app.clone(), &owner, &format!("{detail_path}/artifacts"), serde_json::json!({"upload_id":next_upload["upload_id"],"expected_last_operation":observed["last_operation"],"operation_key":"add"}), StatusCode::OK).await;
    json(app.clone(), &owner, &format!("{detail_path}/uninstall"), serde_json::json!({"expected_last_operation":observed["last_operation"],"operation_key":"stale-delete"}), StatusCode::CONFLICT).await;
    let revoke = json(app.clone(), &owner, &format!("{detail_path}/revoke"), serde_json::json!({"expected_last_operation":add["operation_id"],"operation_key":"revoke","artifact_digest":upload["artifact"]["artifact_digest"],"reason":"withdrawn"}), StatusCode::OK).await;
    assert!(
        revoke["installation"]["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["revoked"] == true)
    );
    let removed = json(app.clone(), &owner, &format!("{detail_path}/uninstall"), serde_json::json!({"expected_last_operation":revoke["operation_id"],"operation_key":"remove"}), StatusCode::OK).await;
    assert_eq!(removed["installation"]["uninstalled"], true);
    // Historical receipts remain historical after catalog movement.
    assert_eq!(
        json(
            app.clone(),
            &owner,
            &catalog,
            install.clone(),
            StatusCode::OK
        )
        .await,
        original
    );
    api.config_store
        .with_conn(|conn| {
            conn.execute(
                "UPDATE api_tokens SET revoked_at='2026-01-01' WHERE token_hash=?1",
                [crate::auth::token_hash(&owner)],
            )?;
            Ok(())
        })
        .unwrap();
    json(
        app.clone(),
        &owner,
        &catalog,
        install,
        StatusCode::UNAUTHORIZED,
    )
    .await;
    shutdown_workspace_feature_storage(&api).await.unwrap();
}

#[tokio::test]
async fn catalog_rejects_invalid_uploads_self_reported_roles_and_runtime_proofs() {
    let dir = tempfile::tempdir().unwrap();
    let mut api = test_api(dir.path()).await;
    let token = seed_test_workspace_owner_token(api.store.as_ref(), "plugin-input").await;
    let identity = RuntimeIdentityMaterial::generate("runtime-plugin-input").unwrap();
    configure_runtime_request_auth(&mut api, &identity, "runtime-plugin-input");
    let app = build_router(api.clone());
    let prefix = format!("/api/w/{TEST_WORKSPACE_ID}");
    request(
        app.clone(),
        Some(&token),
        "POST",
        &format!("{prefix}/plugin-uploads"),
        b"partial zip".to_vec(),
        true,
        StatusCode::BAD_REQUEST,
    )
    .await;
    let max = manifest::plugin::PluginPackageLimits::default().max_package_size_bytes as usize;
    request(
        app.clone(),
        Some(&token),
        "POST",
        &format!("{prefix}/plugin-uploads"),
        vec![0; max + 1],
        true,
        StatusCode::PAYLOAD_TOO_LARGE,
    )
    .await;
    json(app.clone(), &token, &format!("{prefix}/plugins"), serde_json::json!({"upload_id":"missing","alias":"bad","operation_key":"bad","role":"owner"}), StatusCode::BAD_REQUEST).await;
    let response = app
        .clone()
        .oneshot(runtime_source_request(
            &identity,
            None,
            "GET",
            &format!("{prefix}/plugins"),
            vec![],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        get(
            app,
            Some(&token),
            &format!("{prefix}/plugins"),
            StatusCode::OK
        )
        .await["installations"],
        serde_json::json!([])
    );
    shutdown_workspace_feature_storage(&api).await.unwrap();
}

#[tokio::test]
async fn generated_client_and_interrupted_binary_transport_use_real_catalog_authority() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let dir = tempfile::tempdir().unwrap();
    let api = test_api(dir.path()).await;
    let token = seed_test_workspace_owner_token(api.store.as_ref(), "plugin-transport").await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = build_router(api.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    let header = format!(
        "POST /api/w/{TEST_WORKSPACE_ID}/plugin-uploads HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/octet-stream\r\nContent-Length: 2000\r\nConnection: close\r\n\r\npartial"
    );
    stream.write_all(header.as_bytes()).await.unwrap();
    stream.shutdown().await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        stream.read_to_end(&mut response),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        api.config_store
            .with_conn(|conn| Ok(conn.query_row(
                "SELECT count(*) FROM plugin_uploads",
                [],
                |r| r.get::<_, u64>(0)
            )?))
            .unwrap(),
        0
    );
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    let client = server_api::ServerApiClient::builder(format!("http://{address}"))
        .unwrap()
        .client(
            reqwest::Client::builder()
                .default_headers(headers)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
        )
        .build()
        .unwrap();
    let upload = client
        .plugin_upload(TEST_WORKSPACE_ID.into(), package_bytes("0.1.0").into())
        .await
        .unwrap();
    let installed = client
        .plugin_install(
            TEST_WORKSPACE_ID.into(),
            server_api::PluginInstallRequest {
                upload_id: upload.upload_id,
                alias: "transport".into(),
                operation_key: "transport-install".into(),
            },
        )
        .await
        .unwrap();
    let observed = client
        .plugin_show(
            TEST_WORKSPACE_ID.into(),
            installed.installation.installation_id,
        )
        .await
        .unwrap();
    assert_eq!(observed.last_operation, installed.operation_id);
    assert_eq!(observed.artifacts[0], upload.artifact);
    task.abort();
    let _ = task.await;
    shutdown_workspace_feature_storage(&api).await.unwrap();
}

#[tokio::test]
async fn api_restart_recovers_orphan_publication_and_preserves_committed_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let api = test_api(dir.path()).await;
    let token = seed_test_workspace_owner_token(api.store.as_ref(), "plugin-restart").await;
    let app = build_router(api.clone());
    let prefix = format!("/api/w/{TEST_WORKSPACE_ID}");
    let upload = request(
        app.clone(),
        Some(&token),
        "POST",
        &format!("{prefix}/plugin-uploads"),
        package_bytes("0.1.0"),
        true,
        StatusCode::OK,
    )
    .await;
    let installed = json(app.clone(),&token,&format!("{prefix}/plugins"),serde_json::json!({"upload_id":upload["upload_id"],"alias":"restart","operation_key":"restart-install"}),StatusCode::OK).await;
    let root = ServerConfig::server_data_root_for_data_dir(&api.config.data_root)
        .join("plugin-artifacts/blobs");
    let orphan = root.join("f".repeat(64));
    let partial = root.join(".pending-interrupted-restart");
    fs::write(&orphan, b"published without committed reference").unwrap();
    fs::write(&partial, b"partial").unwrap();
    drop(app);
    shutdown_workspace_feature_storage(&api).await.unwrap();
    drop(api);
    let reopened = test_api(dir.path()).await;
    assert!(!orphan.exists());
    assert!(!partial.exists());
    let observed = reopened
        .config_store
        .plugin_show(
            TEST_WORKSPACE_ID,
            installed["installation"]["installation_id"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
    assert_eq!(observed.last_operation, installed["operation_id"]);
    let bytes = reopened
        .config_store
        .plugin_artifact_bytes(
            root.parent().unwrap(),
            TEST_WORKSPACE_ID,
            &observed.installation_id,
            &observed.artifacts[0].artifact_digest,
        )
        .unwrap();
    assert_eq!(bytes, package_bytes("0.1.0"));
    shutdown_workspace_feature_storage(&reopened).await.unwrap();
}
