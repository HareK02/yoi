//! Loopback integration: production RemoteWorkdirSession, signed HTTP middleware,
//! command operation handler, local process pipes, and JSON DTOs. Only opening the
//! pre-seeded session is a fixture; no Runtime Worker/materializer is claimed here.

use super::*;
use std::time::Duration;
use workdir::http::{
    RemoteWorkdirSession, WorkdirHttpAuthorization, WorkdirHttpAuthorizationFuture,
};
use workdir::{CommandOutputRequest, CommandRequest, CommandStream, WorkdirSession};

struct SignedAuthorization {
    identity: RuntimeIdentityMaterial,
    trust: WorkspaceIssuerTrustRecord,
}
impl std::fmt::Debug for SignedAuthorization {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SignedAuthorization")
    }
}
impl WorkdirHttpAuthorization for SignedAuthorization {
    fn bearer_token(
        &self,
        method: String,
        path: String,
        body: Vec<u8>,
    ) -> WorkdirHttpAuthorizationFuture {
        let claims = signed_claims(&self.trust, &method, &path, &body);
        let token =
            issue_workspace_capability_token(&self.identity.signing_key().unwrap(), &claims)
                .unwrap();
        Box::pin(async move { Ok(token) })
    }
}

struct Harness {
    server: tokio::task::JoinHandle<()>,
    session: WorkdirSessionHandle,
    _root: tempfile::TempDir,
    remote: RemoteWorkdirSession,
    client: reqwest::Client,
    operation_url: url::Url,
    authorization: Arc<SignedAuthorization>,
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Harness {
    async fn raw_operation(&self, operation: serde_json::Value) -> reqwest::Response {
        let body = serde_json::to_vec(&serde_json::json!({"operation": operation})).unwrap();
        let token = self
            .authorization
            .bearer_token(
                "POST".into(),
                self.operation_url.path().into(),
                body.clone(),
            )
            .await
            .unwrap();
        self.client
            .post(self.operation_url.clone())
            .bearer_auth(token)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .unwrap()
    }

    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let session: WorkdirSessionHandle = Arc::new(LocalWorkdirSession::materialized_bound(
            Workdir::new("wd-transport"),
            root.path().to_path_buf(),
            root.path().to_path_buf(),
            SharedScope::new(Scope::writable(root.path()).unwrap()),
            WorkdirSessionCapabilities::ALL,
        ));
        let identity = RuntimeIdentityMaterial::generate("transport-key").unwrap();
        let trust = workspace_trust(&identity);
        let state = RuntimeHttpState {
            runtime: Runtime::new_memory(),
            local_token: None,
            workspace_auth: Some(Arc::new(workspace_test_auth(vec![trust.clone()]))),
            workdir_sessions: Arc::new(Mutex::new(HashMap::from([(
                "session-transport".into(),
                RuntimeHttpWorkdirSession {
                    owner: RuntimeWorkspaceScope::new("workspace-a", "https://backend.test"),
                    session: session.clone(),
                },
            )]))),
            ssh_keyscan_program: Arc::new(PathBuf::from("ssh-keyscan")),
        };
        let app = Router::new()
            .route(
                "/v1/working-directories/{id}/sessions",
                post(|| async {
                    Json(OpenWorkdirSessionResponse {
                        session_id: WorkdirSessionId::new("session-transport").unwrap(),
                        workdir_id: Workdir::new("wd-transport").id().clone(),
                        capabilities: WorkdirSessionCapabilities::ALL,
                    })
                }),
            )
            .route(
                "/v1/workdir-sessions/{session_id}/operations",
                post(run_workdir_session_operation),
            )
            .route(
                "/v1/workdir-sessions/{session_id}",
                delete(close_workdir_session),
            )
            .with_state(state.clone())
            .layer(middleware::from_fn_with_state(state, require_runtime_auth));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base: url::Url = format!("http://{}", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            RUNTIME_WORKSPACE_SCOPE_HEADER,
            "workspace-a".parse().unwrap(),
        );
        let client = reqwest::Client::builder()
            .no_proxy()
            .default_headers(headers)
            .build()
            .unwrap();
        let authorization = Arc::new(SignedAuthorization { identity, trust });
        let operation_url = base
            .join("/v1/workdir-sessions/session-transport/operations")
            .unwrap();
        let remote = RemoteWorkdirSession::open_with_authorization(
            client.clone(),
            base,
            authorization.clone(),
            Workdir::new("wd-transport").id().clone(),
            OpenWorkdirSessionRequest::default(),
        )
        .await
        .unwrap();
        Self {
            server,
            session,
            _root: root,
            remote,
            client,
            operation_url,
            authorization,
        }
    }
}

#[tokio::test]
async fn real_http_closed_session_retains_failure_reason() {
    let harness = Harness::new().await;
    let handle = harness
        .remote
        .start_command(CommandRequest {
            transport_byte_budget: 1024,
            command: "printf done".into(),
            timeout_secs: 5,
            cwd: WorkdirPath::root(),
            tool_call_id: None,
        })
        .await
        .unwrap();
    // Keep the HTTP session identity registered, but close the underlying provider.
    harness.session.close().await.unwrap();
    let error = harness
        .remote
        .command_output(CommandOutputRequest {
            handle,
            cursor: 0,
            limit: 16,
            wait: true,
        })
        .await
        .unwrap_err();
    assert_eq!(
        error.failure_reason(),
        Some(workdir::WorkdirFailureReason::SessionClosed)
    );
    assert!(matches!(
        error.classification_source(),
        workdir::WorkdirError::Unavailable(_)
    ));
}

#[tokio::test]
async fn real_http_command_pages_preserve_binary_streams_replay_and_nonzero_exit() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let harness = Harness::new().await;
        let handle = harness.remote.start_command(CommandRequest {
            transport_byte_budget: workdir::MAX_COMMAND_TRANSPORT_BYTES,
            command: "head -c 2097152 /dev/zero | tr '\\0' x; printf '\\342\\202\\254\\377\\000'; printf 'stderr\\346\\227\\245' >&2; exit 9".into(),
            timeout_secs: 15, cwd: WorkdirPath::root(), tool_call_id: Some("http-tool-call".into()),
        }).await.unwrap();
        let mut cursor = 0;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let terminal = loop {
            let request = CommandOutputRequest { handle: handle.clone(), cursor, limit: 16384, wait: true };
            let page = harness.remote.command_output(request.clone()).await.unwrap();
            assert_eq!(harness.remote.command_output(request).await.unwrap(), page);
            assert!(page.chunks.iter().map(|chunk| chunk.bytes.len()).sum::<usize>() <= 16384);
            for chunk in page.chunks {
                assert_eq!(chunk.sequence, cursor);
                cursor += chunk.bytes.len() as u64;
                let target = match chunk.stream { CommandStream::Stdout => &mut stdout, CommandStream::Stderr => &mut stderr };
                assert_eq!(chunk.offset, target.len() as u64);
                target.extend(chunk.bytes);
            }
            assert_eq!(cursor, page.next_cursor);
            if let Some(terminal) = page.terminal { break terminal; }
        };
        let mut expected = vec![b'x'; 2097152];
        expected.extend_from_slice(b"\xe2\x82\xac\xff\0");
        assert_eq!(stdout, expected);
        assert_eq!(stderr, b"stderr\xe6\x97\xa5");
        assert_eq!(terminal.status, workdir::CommandStatus::Failed);
        assert_eq!(terminal.exit_code, Some(9));
        assert!(harness.remote.command_output(CommandOutputRequest { handle, cursor: 0, limit: 1, wait: false }).await.is_err());
        harness.remote.close().await.unwrap();
        assert!(harness.session.command_snapshot().is_empty());
    }).await.expect("bounded HTTP command transport");
}

#[tokio::test]
async fn real_http_cancel_drains_tail_before_terminal_and_timeout_is_distinct() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let harness = Harness::new().await;
        for (command, timeout, cancel, expected) in [
            (
                "printf ready; exec yes x",
                10,
                true,
                workdir::CommandStatus::Cancelled,
            ),
            (
                "printf ready; exec tail -f /dev/null",
                1,
                false,
                workdir::CommandStatus::TimedOut,
            ),
        ] {
            let handle = harness
                .remote
                .start_command(CommandRequest {
                    transport_byte_budget: workdir::MAX_COMMAND_TRANSPORT_BYTES,
                    command: command.into(),
                    timeout_secs: timeout,
                    cwd: WorkdirPath::root(),
                    tool_call_id: None,
                })
                .await
                .unwrap();
            let mut cursor = 0;
            let first = harness
                .remote
                .command_output(CommandOutputRequest {
                    handle: handle.clone(),
                    cursor,
                    limit: 5,
                    wait: true,
                })
                .await
                .unwrap();
            assert_eq!(
                first
                    .chunks
                    .iter()
                    .flat_map(|chunk| chunk.bytes.iter().copied())
                    .collect::<Vec<_>>(),
                b"ready"
            );
            cursor = first.next_cursor;
            if cancel {
                harness.remote.cancel_command(handle.clone()).await.unwrap();
            }
            loop {
                let page = harness
                    .remote
                    .command_output(CommandOutputRequest {
                        handle: handle.clone(),
                        cursor,
                        limit: workdir::MAX_COMMAND_READ_BYTES,
                        wait: true,
                    })
                    .await
                    .unwrap();
                cursor = page.next_cursor;
                if let Some(terminal) = page.terminal {
                    assert_eq!(terminal.status, expected);
                    break;
                }
            }
        }
        harness.remote.close().await.unwrap();
    })
    .await
    .expect("bounded cancellation and timeout over HTTP");
}

#[tokio::test]
async fn real_http_command_budget_bounds_unique_payload_and_reports_larger_seen_totals() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let harness = Harness::new().await;
        for budget in [0, 12345] {
            let handle = harness
                .remote
                .start_command(CommandRequest {
                    command: "head -c 1048576 /dev/zero; head -c 65536 /dev/zero >&2; exit 9"
                        .into(),
                    timeout_secs: 10,
                    transport_byte_budget: budget,
                    cwd: WorkdirPath::root(),
                    tool_call_id: None,
                })
                .await
                .unwrap();
            let mut cursor = 0;
            let mut delivered = [0_u64; 2];
            let terminal = loop {
                let request = CommandOutputRequest {
                    handle: handle.clone(),
                    cursor,
                    limit: 997,
                    wait: true,
                };
                let page = harness
                    .remote
                    .command_output(request.clone())
                    .await
                    .unwrap();
                assert_eq!(harness.remote.command_output(request).await.unwrap(), page);
                for chunk in page.chunks {
                    assert_eq!(chunk.sequence, cursor);
                    cursor += chunk.bytes.len() as u64;
                    let index = match chunk.stream {
                        CommandStream::Stdout => 0,
                        CommandStream::Stderr => 1,
                    };
                    assert_eq!(chunk.offset, delivered[index]);
                    delivered[index] += chunk.bytes.len() as u64;
                }
                assert_eq!(cursor, page.next_cursor);
                assert!(
                    cursor <= budget,
                    "RemoteWorkdirSession must not receive bytes beyond budget"
                );
                if let Some(terminal) = page.terminal {
                    break terminal;
                }
            };
            assert_eq!(cursor, budget);
            assert_eq!(terminal.stdout_bytes, 1048576);
            assert_eq!(terminal.stderr_bytes, 65536);
            assert_eq!(terminal.stdout_delivered_bytes, delivered[0]);
            assert_eq!(terminal.stderr_delivered_bytes, delivered[1]);
            assert_eq!(
                terminal.truncation_reason,
                Some(workdir::CommandOutputTruncation::ByteBudget)
            );
            assert_eq!(terminal.status, workdir::CommandStatus::Failed);
            assert_eq!(terminal.exit_code, Some(9));
            assert!(
                harness
                    .remote
                    .command_output(CommandOutputRequest {
                        handle: handle.clone(),
                        cursor: budget + 1,
                        limit: 1,
                        wait: false,
                    })
                    .await
                    .is_err()
            );
            if budget > 0 {
                assert!(
                    harness
                        .remote
                        .command_output(CommandOutputRequest {
                            handle,
                            cursor: 0,
                            limit: 1,
                            wait: false,
                        })
                        .await
                        .is_err()
                );
            }
        }
        harness.remote.close().await.unwrap();
    })
    .await
    .expect("bounded HTTP byte-budget transport");
}

#[tokio::test]
async fn real_http_rejects_invalid_or_legacy_budget_requests_before_process_start() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let harness = Harness::new().await;
        let request = CommandRequest {
            command: "touch must-not-exist".into(),
            timeout_secs: 1,
            transport_byte_budget: workdir::MAX_COMMAND_TRANSPORT_BYTES + 1,
            cwd: WorkdirPath::root(),
            tool_call_id: None,
        };
        let oversize =
            serde_json::to_value(WorkdirSessionOperation::CommandStart(request)).unwrap();
        assert_eq!(
            harness.raw_operation(oversize.clone()).await.status(),
            reqwest::StatusCode::BAD_REQUEST
        );
        let mut missing = oversize.clone();
        // CommandStart is externally tagged by operation / request.
        let object = missing.as_object_mut().unwrap();
        let request = object.get_mut("request").unwrap().as_object_mut().unwrap();
        request.remove("transport_byte_budget");
        assert_eq!(
            harness.raw_operation(missing).await.status(),
            reqwest::StatusCode::BAD_REQUEST
        );
        let mut legacy = oversize;
        let request = legacy["request"].as_object_mut().unwrap();
        request.insert("transport_byte_budget".into(), serde_json::json!(0));
        request.insert("spill_dir".into(), serde_json::json!("/private/host"));
        assert_eq!(
            harness.raw_operation(legacy).await.status(),
            reqwest::StatusCode::BAD_REQUEST
        );
        assert!(!harness._root.path().join("must-not-exist").exists());
        harness.remote.close().await.unwrap();
    })
    .await
    .expect("bounded HTTP budget admission");
}
