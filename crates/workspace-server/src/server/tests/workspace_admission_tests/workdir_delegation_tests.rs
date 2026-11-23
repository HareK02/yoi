//! Ordinary library integration tests: the production Worker attachment client,
//! signed Workspace HTTP router, live operation authorization, external transport
//! and pinned local provider are connected. Only Runtime Worker execution and the
//! client-to-router socket are replaced; the admission fixture owns isolated DBs.
use super::super::workdir_delegation_tests::verify_backend_delegation;
use super::*;
use workdir::{
    BoundedReadLimits, LocalWorkdirSession, Workdir, WorkdirSessionCapabilities as Caps,
};
use worker::feature::builtin::manage_workdir::WorkspaceAttachedWorkdirSession;
use worker::{WorkspaceClient, WorkspaceClientError, WorkspaceRequest, WorkspaceResponse};

struct SignedClient {
    fixture: Arc<Fixture>,
    handle: tokio::runtime::Handle,
}

impl std::fmt::Debug for SignedClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignedWorkdirClient")
            .finish_non_exhaustive()
    }
}

impl WorkspaceClient for SignedClient {
    fn workspace_id(&self) -> Option<&str> {
        Some(TEST_WORKSPACE_ID)
    }
    fn kind(&self) -> &str {
        "signed-in-process"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn execute(
        &self,
        request: WorkspaceRequest,
    ) -> std::result::Result<WorkspaceResponse, WorkspaceClientError> {
        assert!(matches!(
            request.method,
            worker::WorkspaceRequestMethod::Post
        ));
        let request = self.fixture.request(
            Some("1"),
            &request.path,
            request.body.unwrap_or_default().into_bytes(),
            START,
            worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
        );
        tokio::task::block_in_place(|| {
            self.handle.block_on(async {
                let response =
                    tokio::time::timeout(BOUND, self.fixture.app.clone().oneshot(request))
                        .await
                        .unwrap()
                        .unwrap();
                let status = response.status().as_u16();
                let content_type = response
                    .headers()
                    .get(CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                let body = to_bytes(response.into_body(), 2 * 1024 * 1024)
                    .await
                    .unwrap();
                Ok(WorkspaceResponse {
                    status,
                    content_type,
                    body: String::from_utf8(body.to_vec()).unwrap(),
                })
            })
        })
    }
}

struct ProviderTask(tokio::task::JoinHandle<()>);
impl Drop for ProviderTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn assert_revoked(response: Response) {
    let status = response.status();
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    // The current grant-lineage contract reports revoked grants as InvalidInput.
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|message| message.contains("Core Worker grant revoked")),
        "{body}"
    );
}

async fn run_with_external_provider(capabilities: Caps) {
    let f = Arc::new(Fixture::new().await);
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("child")).unwrap();
    fs::create_dir(root.path().join("other")).unwrap();
    fs::write(root.path().join("child/input"), "visible").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.path().join("child"), root.path().join("alias")).unwrap();
    let local = LocalWorkdirSession::external_with_capabilities_pinned(
        Workdir::new("external-1"),
        workdir::ExternalWorkdirRoot::pin(root.path()).unwrap(),
        BoundedReadLimits::new(4096, 1024).unwrap(),
        capabilities,
    )
    .unwrap();
    let mut commands = f.attach_with_capabilities("1", capabilities);
    let comparisons = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = comparisons.clone();
    let _provider = ProviderTask(tokio::spawn(async move {
        while let Some(command) = commands.recv().await {
            let ExternalProviderCommand::Operation {
                operation,
                response,
                ..
            } = command
            else {
                panic!("unexpected provider control command");
            };
            if matches!(&operation, WorkdirSessionOperation::ScopeRulesOverlap(_)) {
                observed.fetch_add(1, Ordering::SeqCst);
            }
            let result = workdir::dispatch_workdir_session_operation(&local, operation)
                .await
                .map_err(|error| WorkdirTransportError::from_workdir_error(&error));
            let _ = response.send(result);
        }
    }));
    let client = Arc::new(SignedClient {
        fixture: f.clone(),
        handle: tokio::runtime::Handle::current(),
    });
    let source = WorkspaceAttachedWorkdirSession::handle_for_workdir_with_capabilities(
        client,
        "attachment",
        "external-1",
        capabilities,
    );
    // The shared contract checks two writable siblings, conflict rejection,
    // parent exclusion and lease release through the Server operation entrypoint.
    tokio::time::timeout(
        std::time::Duration::from_secs(20),
        verify_backend_delegation(source, capabilities),
    )
    .await
    .unwrap();
    assert!(comparisons.load(Ordering::SeqCst) > 0);
    if capabilities == Caps::READ_WRITE {
        assert_eq!(
            fs::read(root.path().join("child/created")).unwrap(),
            b"written"
        );
        assert_eq!(
            fs::read(root.path().join("other/created")).unwrap(),
            b"written"
        );
        assert!(!root.path().join("child/blocked").exists());
    } else {
        assert!(!root.path().join("child/created").exists());
    }

    let overlap = |left: &str| {
        serde_json::from_value::<WorkdirSessionOperation>(json!({
            "operation": "scope_rules_overlap", "request": {
                "left": {"target": left, "permission": "write", "recursive": true},
                "right": {"target": "child", "permission": "write", "recursive": true}
            }
        }))
        .unwrap()
    };
    let send = |worker: &str, operation| {
        let request = f.request(
            Some(worker),
            &format!("/api/w/{TEST_WORKSPACE_ID}/workers/self/workdir-session/operations"),
            serde_json::to_vec(&WorkspaceWorkdirSessionOperationRequest {
                target_workdir: "attachment".into(),
                operation,
            })
            .unwrap(),
            START,
            worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
        );
        f.app.clone().oneshot(request)
    };
    let response = send("1", overlap("child")).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: WorkdirSessionOperationResult =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(
        body,
        WorkdirSessionOperationResult::ScopeRulesOverlap { overlaps: true }
    );
    #[cfg(unix)]
    {
        // Resolution must detect aliases as overlapping, not just compare text.
        // That observation does not authorize file access through the symlink.
        let response = send("1", overlap("alias")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: WorkdirSessionOperationResult =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(
            body,
            WorkdirSessionOperationResult::ScopeRulesOverlap { overlaps: true }
        );
        let response = send(
            "1",
            WorkdirSessionOperation::Read(workdir::ReadRequest {
                path: workdir::WorkdirPath::new("alias/input").unwrap(),
                offset: 0,
                limit: 10,
                max_bytes: 1024,
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    let before = comparisons.load(Ordering::SeqCst);
    assert!(
        !send("2", overlap("child"))
            .await
            .unwrap()
            .status()
            .is_success()
    );
    assert_eq!(
        comparisons.load(Ordering::SeqCst),
        before,
        "another Worker cannot use this attachment"
    );
    let grant = f
        .api
        .config_store
        .core_worker_execution_grant(
            TEST_WORKSPACE_ID,
            WorkdirlessFixtureRuntime::RUNTIME_ID,
            "1",
        )
        .unwrap()
        .unwrap();
    f.api
        .config_store
        .revoke_core_worker_execution_grant(
            TEST_WORKSPACE_ID,
            &test_owner_actor().account_id,
            &grant.grant_id,
            "revoke-scope-comparison",
        )
        .unwrap();
    assert_revoked(send("1", overlap("child")).await.unwrap()).await;
    assert_eq!(
        comparisons.load(Ordering::SeqCst),
        before,
        "revocation must reject before provider comparison"
    );
    let response = f.app.clone().oneshot(f.operation("1")).await.unwrap();
    assert_revoked(response).await;
    let response = send(
        "1",
        WorkdirSessionOperation::Write(workdir::WriteRequest {
            path: workdir::WorkdirPath::new("after-revoke").unwrap(),
            content: b"denied".to_vec(),
            expected_hash: None,
        }),
    )
    .await
    .unwrap();
    assert_revoked(response).await;
    assert!(!root.path().join("after-revoke").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signed_workdir_route_supports_sibling_write_leases_and_live_revocation() {
    run_with_external_provider(Caps::READ_WRITE).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signed_readonly_workdir_can_compare_write_rules_but_cannot_delegate_writes() {
    run_with_external_provider(Caps::READ_ONLY).await;
}
