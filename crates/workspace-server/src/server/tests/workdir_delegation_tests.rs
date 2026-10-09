use super::*;
use workdir::{
    BoundedReadLimits, LocalWorkdirSession, Workdir, WorkdirPath,
    WorkdirSessionCapabilities as Caps, WorkdirToolBroker, WorkdirToolScope,
    WorkdirToolScopePermission as Permission, WorkdirToolScopeRule,
};

fn scope(target: &str, permission: Permission) -> WorkdirToolScope {
    WorkdirToolScope {
        rules: vec![WorkdirToolScopeRule {
            target: WorkdirPath::new(target).unwrap(),
            permission,
            recursive: true,
            symlink_policy: Default::default(),
        }],
        cwd: WorkdirPath::new(target).unwrap(),
        command: false,
    }
}

async fn verify_backend_delegation(source: WorkdirSessionHandle, capabilities: Caps) {
    // The same capability wrapper returned by both Backend attachment branches.
    let backend = WorkdirToolBroker::with_capabilities(source, capabilities);
    let session = backend.tool_session();
    let authorization = workdir::WorkdirScopeAuthorizationRequest {
        rules: scope("child", Permission::Read).rules,
        path: WorkdirPath::new("child").unwrap(),
        permission: Permission::Read,
    };
    assert!(matches!(
        execute_workdir_session_operation(
            &session,
            WorkdirSessionOperation::AuthorizeScope(authorization)
        )
        .await
        .unwrap(),
        WorkdirSessionOperationResult::AuthorizeScope
    ));
    let worker = WorkdirToolBroker::new(session.clone());
    let child = worker
        .scope(scope("child", Permission::Read))
        .await
        .unwrap();
    assert_eq!(
        child
            .read(workdir::ReadRequest {
                path: WorkdirPath::new("input").unwrap(),
                offset: 0,
                limit: 10,
                max_bytes: 1024,
            })
            .await
            .unwrap()
            .bytes,
        b"visible"
    );
    child.close().await.unwrap();

    // Comparison is read-only even when the compared rules describe writes.
    let overlap = workdir::WorkdirScopeOverlapRequest {
        left: scope("child", Permission::Write).rules.remove(0),
        right: scope("other", Permission::Write).rules.remove(0),
    };
    assert!(matches!(
        execute_workdir_session_operation(
            &session,
            WorkdirSessionOperation::ScopeRulesOverlap(overlap)
        )
        .await
        .unwrap(),
        WorkdirSessionOperationResult::ScopeRulesOverlap { overlaps: false }
    ));
    let writer = worker.scope(scope("child", Permission::Write)).await;
    if !capabilities.supports(workdir::WorkdirSessionCapability::Write) {
        assert!(writer.is_err());
        return;
    }
    let writer = writer.unwrap();
    let sibling = worker
        .scope(scope("other", Permission::Write))
        .await
        .unwrap();
    let write = |path: &str| workdir::WriteRequest {
        path: WorkdirPath::new(path).unwrap(),
        content: b"written".to_vec(),
        expected_hash: None,
    };
    writer.write(write("created")).await.unwrap();
    assert!(worker.write(write("child/blocked")).await.is_err());
    assert!(
        worker
            .scope(scope("child", Permission::Write))
            .await
            .is_err()
    );
    sibling.write(write("created")).await.unwrap();
    writer.close().await.unwrap();
    worker.write(write("child/released")).await.unwrap();
    sibling.close().await.unwrap();
}

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("child")).unwrap();
    fs::create_dir(root.path().join("other")).unwrap();
    fs::write(root.path().join("child/input"), "visible").unwrap();
    root
}

#[tokio::test]
async fn backend_delegation_reaches_external_provider_resolution() {
    for capabilities in [Caps::READ_ONLY, Caps::READ_WRITE] {
        let root = fixture();
        let local = LocalWorkdirSession::external_with_capabilities_pinned(
            Workdir::new("external-delegation"),
            workdir::ExternalWorkdirRoot::pin(root.path()).unwrap(),
            BoundedReadLimits::new(4096, 1024).unwrap(),
            capabilities,
        )
        .unwrap();
        let (sender, mut commands) = tokio::sync::mpsc::channel(16);
        let source = Arc::new(ExternalProviderWorkdirSession::new(
            Arc::new(ExternalProviderConnection {
                grant_id: "delegation-grant".into(),
                workdir_id: "external-delegation".into(),
                provider_instance_id: "delegation-provider".into(),
                generation: 1,
                expires_at: None,
                capabilities,
                admission: Arc::new(tokio::sync::Semaphore::new(16)),
                shutdown_confirmed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                sender,
            }),
            None,
        ));
        // Real external session transport and real provider authorization, not
        // canned allow responses. Wire serialization also preserves the rules.
        let provider = tokio::spawn(async move {
            let mut authorizations = 0;
            let mut overlaps = 0;
            while let Some(command) = commands.recv().await {
                let ExternalProviderCommand::Operation {
                    operation,
                    response,
                    ..
                } = command
                else {
                    panic!("unexpected provider control command");
                };
                let operation =
                    serde_json::from_slice(&serde_json::to_vec(&operation).unwrap()).unwrap();
                match &operation {
                    WorkdirSessionOperation::AuthorizeScope(_) => authorizations += 1,
                    WorkdirSessionOperation::ScopeRulesOverlap(_) => overlaps += 1,
                    _ => {}
                }
                let result = workdir::dispatch_workdir_session_operation(&local, operation)
                    .await
                    .map_err(|error| WorkdirTransportError::from_workdir_error(&error));
                response.send(result).unwrap();
            }
            (authorizations, overlaps)
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            verify_backend_delegation(source, capabilities),
        )
        .await
        .unwrap();
        let (authorizations, overlaps) =
            tokio::time::timeout(std::time::Duration::from_secs(5), provider)
                .await
                .unwrap()
                .unwrap();
        assert!(authorizations >= 3);
        assert!(overlaps >= 1);
    }
}

#[tokio::test]
async fn backend_delegation_reaches_remote_repository_resolution() {
    use workdir::http::{
        OpenWorkdirSessionResponse, RemoteWorkdirSession, WorkdirSessionId,
        WorkdirSessionOperationRequest,
    };
    let root = fixture();
    let source: WorkdirSessionHandle = Arc::new(LocalWorkdirSession::new(
        manifest::Scope::writable(root.path()).unwrap(),
        root.path().to_path_buf(),
    ));
    let workdir_id = source.workdir().id().clone();
    let response_id = workdir_id.clone();
    let app = Router::new()
        .route(
            "/v1/working-directories/{id}/sessions",
            axum::routing::post(move || {
                let workdir_id = response_id.clone();
                async move {
                    Json(OpenWorkdirSessionResponse {
                        session_id: WorkdirSessionId::new("delegation-session").unwrap(),
                        workdir_id,
                        capabilities: Caps::ALL,
                    })
                }
            }),
        )
        .route(
            "/v1/workdir-sessions/{id}/operations",
            axum::routing::post(move |Json(request): Json<WorkdirSessionOperationRequest>| {
                let source = source.clone();
                async move {
                    match workdir::dispatch_workdir_session_operation(
                        source.as_ref(),
                        request.operation,
                    )
                    .await
                    {
                        Ok(result) => Json(result).into_response(),
                        Err(error) => {
                            let error = WorkdirTransportError::from_workdir_error(&error);
                            (
                                StatusCode::from_u16(error.code.http_status()).unwrap(),
                                Json(error),
                            )
                                .into_response()
                        }
                    }
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown, finished) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = finished.await;
            })
            .await
            .unwrap();
    });
    let remote = RemoteWorkdirSession::open(
        reqwest::Client::new(),
        format!("http://{address}").parse().unwrap(),
        "test-session-token",
        workdir_id,
        Default::default(),
    )
    .await
    .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        verify_backend_delegation(Arc::new(remote), Caps::ALL),
    )
    .await
    .unwrap();
    shutdown.send(()).unwrap();
    server.await.unwrap();
}
