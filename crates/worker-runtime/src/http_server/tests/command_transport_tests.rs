//! Loopback integration: production RemoteWorkdirSession, signed HTTP middleware,
//! command operation handler, local process pipes, and JSON DTOs. Only opening the
//! pre-seeded session is a fixture; no Runtime Worker/materializer is claimed here.

use super::*;
use std::collections::BTreeMap;
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
        Self::with_environment(tempfile::tempdir().unwrap(), BTreeMap::new()).await
    }

    async fn with_environment(
        root: tempfile::TempDir,
        command_environment: BTreeMap<String, String>,
    ) -> Self {
        let session: WorkdirSessionHandle =
            Arc::new(LocalWorkdirSession::materialized_bound_with_environment(
                Workdir::new("wd-transport"),
                root.path().to_path_buf(),
                root.path().to_path_buf(),
                SharedScope::new(Scope::writable(root.path()).unwrap()),
                WorkdirSessionCapabilities::ALL,
                command_environment,
                Vec::new(),
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
                        environment: workdir::WorkdirSessionEnvironment::root(
                            WorkdirSessionCapabilities::ALL,
                        ),
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
async fn real_http_waiting_command_survives_long_poll_expiry() {
    tokio::time::timeout(Duration::from_secs(45), async {
        let harness = Harness::new().await;
        let handle = harness
            .remote
            .start_command(CommandRequest {
                transport_byte_budget: 1024,
                command: "printf ready; while [ ! -e release ]; do sleep 0.05; done; printf after; printf err >&2; exit 7".into(),
                timeout_secs: 40,
                cwd: WorkdirPath::root(),
                tool_call_id: None,
            })
            .await
            .unwrap();
        let mut request = CommandOutputRequest {
            handle,
            cursor: 0,
            limit: 1024,
            wait: true,
        };
        let ready = harness.remote.command_output(request.clone()).await.unwrap();
        assert_eq!(ready.chunks.iter().flat_map(|c| c.bytes.iter().copied()).collect::<Vec<_>>(), b"ready");
        assert!(ready.terminal.is_none());
        request.cursor = ready.next_cursor;

        // Keep the command silent until the production HTTP handler has expired
        // its 20-second long poll. The client must keep waiting, not fail or
        // expose an empty page as completed output.
        let waiting = harness.remote.command_output(request.clone());
        tokio::pin!(waiting);
        tokio::select! {
            result = &mut waiting => panic!("waiting read ended before release: {result:?}"),
            _ = tokio::time::sleep(Duration::from_secs(21)) => {}
        }
        std::fs::write(harness._root.path().join("release"), b"").unwrap();
        let mut page = waiting.await.unwrap();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let terminal = loop {
            // The retry cursor still replays the exact nonempty/terminal page.
            assert_eq!(harness.remote.command_output(request.clone()).await.unwrap(), page);
            for chunk in &page.chunks {
                assert_eq!(chunk.sequence, request.cursor);
                request.cursor += chunk.bytes.len() as u64;
                match chunk.stream {
                    CommandStream::Stdout => stdout.extend_from_slice(&chunk.bytes),
                    CommandStream::Stderr => stderr.extend_from_slice(&chunk.bytes),
                }
            }
            assert_eq!(page.next_cursor, request.cursor);
            if let Some(terminal) = page.terminal {
                break terminal;
            }
            page = harness.remote.command_output(request.clone()).await.unwrap();
        };
        assert_eq!(stdout, b"after");
        assert_eq!(stderr, b"err");
        assert_eq!(terminal.status, workdir::CommandStatus::Failed);
        assert_eq!(terminal.exit_code, Some(7));
        harness.remote.close().await.unwrap();
    })
    .await
    .expect("bounded long-poll continuation over HTTP");
}

#[tokio::test]
async fn real_http_spawn_failure_preserves_safe_cause_counts_and_terminal_replay() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let root = tempfile::tempdir().unwrap();
        let missing_path = root.path().join("private-spawn-path-must-not-exist");
        assert!(!missing_path.exists());
        let private_path = missing_path.to_str().unwrap().to_owned();
        let environment_secret = "spawn-environment-secret-must-not-leak";
        let command_secret = "spawn-command-secret-must-not-leak";
        let tool_call_secret = "spawn-tool-call-secret-must-not-leak";
        // This is an OS spawn failure in the production local provider, not a
        // fabricated terminal: PATH contains no bash. The test process environment
        // is untouched, so parallel command tests retain their normal PATH.
        let harness = Harness::with_environment(
            root,
            BTreeMap::from([
                ("PATH".into(), private_path.clone()),
                ("PRIVATE_AUTH_TOKEN".into(), environment_secret.into()),
            ]),
        )
        .await;
        let command = format!("printf '{command_secret}'; printf ran > must-not-run");
        let handle = harness
            .remote
            .start_command(CommandRequest {
                transport_byte_budget: 1024,
                command: command.clone(),
                timeout_secs: 5,
                cwd: WorkdirPath::root(),
                tool_call_id: Some(tool_call_secret.into()),
            })
            .await
            .unwrap();
        let request = CommandOutputRequest {
            handle,
            cursor: 0,
            limit: 16,
            wait: true,
        };
        let page = harness.remote.command_output(request.clone()).await.unwrap();
        assert!(page.chunks.is_empty());
        assert_eq!(page.next_cursor, 0);
        let terminal = page.terminal.as_ref().expect("spawn failure is terminal");
        assert_eq!(terminal.status, workdir::CommandStatus::Failed);
        assert_eq!(terminal.exit_code, None);
        assert_eq!(
            terminal.process_failure,
            Some(workdir::CommandFailure {
                stage: workdir::CommandFailureStage::Spawn,
                reason: workdir::CommandIoErrorKind::NotFound,
                os_code: Some(libc::ENOENT),
            })
        );
        assert_eq!(
            terminal.output_collection,
            workdir::CommandOutputCollection::NotStarted
        );
        assert_eq!(terminal.stdout_bytes, 0);
        assert_eq!(terminal.stderr_bytes, 0);
        assert_eq!(terminal.stdout_delivered_bytes, 0);
        assert_eq!(terminal.stderr_delivered_bytes, 0);
        assert_eq!(terminal.truncation_reason, None);

        // A zero-byte terminal page remains replayable at its unchanged cursor;
        // neither a retry nor a smaller nonwaiting read changes its accounting.
        assert_eq!(
            harness.remote.command_output(request.clone()).await.unwrap(),
            page
        );
        let replay_request = CommandOutputRequest {
            wait: false,
            limit: 1,
            ..request
        };
        assert_eq!(
            harness
                .remote
                .command_output(replay_request.clone())
                .await
                .unwrap(),
            page
        );
        let response = harness
            .raw_operation(
                serde_json::to_value(WorkdirSessionOperation::CommandOutput(
                    replay_request.clone(),
                ))
                .unwrap(),
            )
            .await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body = response.text().await.unwrap();
        for secret in [
            private_path.as_str(),
            harness._root.path().to_str().unwrap(),
            "private-spawn-path-must-not-exist",
            "PRIVATE_AUTH_TOKEN",
            environment_secret,
            command_secret,
            command.as_str(),
            tool_call_secret,
        ] {
            assert!(
                !body.contains(secret),
                "private input leaked into HTTP terminal"
            );
        }
        let wire: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({
                "operation": "command_output",
                "result": {
                    "chunks": [],
                    "next_cursor": 0,
                    "terminal": {
                        "status": "failed",
                        "exit_code": null,
                        "process_failure": {"stage": "spawn", "reason": "not_found", "os_code": libc::ENOENT},
                        "output_collection": {"state": "not_started"},
                        "stdout_bytes": 0,
                        "stderr_bytes": 0,
                        "stdout_delivered_bytes": 0,
                        "stderr_delivered_bytes": 0,
                        "truncation_reason": null
                    }
                }
            })
        );
        assert_eq!(
            harness.remote.command_output(replay_request).await.unwrap(),
            page
        );
        // Exercise the same real spawn failure through Bash, not a terminal
        // fixture. Opening another remote client reuses the pre-seeded provider
        // session and still passes through the signed HTTP middleware.
        let bash_remote: WorkdirSessionHandle = Arc::new(
            RemoteWorkdirSession::open_with_authorization(
                harness.client.clone(),
                harness.operation_url.join("/").unwrap(),
                harness.authorization.clone(),
                Workdir::new("wd-transport").id().clone(),
                OpenWorkdirSessionRequest::default(),
            )
            .await
            .unwrap(),
        );
        let view = tools::bash_output::BashOutputView::new();
        let (_, bash) = tools::bash_tool(bash_remote, view.clone())();
        let input = serde_json::json!({
            "command": command,
            "cwd": ".",
            "timeout": 5
        })
        .to_string();
        let output = bash
            .execute(
                &input,
                agen::tool::ToolExecutionContext::new(tool_call_secret, "http-spawn-attempt", 0),
            )
            .await
            .expect("Bash returns a structured failed-command result");
        let content = output.content.as_deref().expect("Bash ToolOutput body");
        let tool_body: serde_json::Value = serde_json::from_str(content).unwrap();
        assert_eq!(tool_body["stdout_preview"], "");
        assert_eq!(tool_body["stderr_preview"], "");
        assert_eq!(tool_body["preview_omitted"], false);
        assert_eq!(
            tool_body["output"]["terminal"],
            serde_json::json!({
                "status": "failed",
                "exit_code": null,
                "process_failure": {"stage": "spawn", "reason": "not_found", "os_code": libc::ENOENT},
                "output_collection": {"state": "not_started"},
                "transport_complete": false
            })
        );
        let output_id = tool_body["output"]["id"].as_str().unwrap();
        let retained = view.metadata(output_id).expect("retained spawn failure");
        assert_eq!(retained.terminal.status, Some(workdir::CommandStatus::Failed));
        assert_eq!(retained.terminal.exit_code, None);
        assert_eq!(retained.terminal.process_failure, terminal.process_failure);
        assert_eq!(
            retained.terminal.output_collection,
            Some(workdir::CommandOutputCollection::NotStarted)
        );
        assert!(!retained.terminal.transport_complete);
        assert_eq!(retained.stdout_saved, 0);
        assert_eq!(retained.stderr_saved, 0);
        assert_eq!(retained.stdout_seen, 0);
        assert_eq!(retained.stderr_seen, 0);
        assert_eq!(retained.stdout_delivered, 0);
        assert_eq!(retained.stderr_delivered, 0);
        assert!(!retained.storage_truncated);
        assert!(retained.retention_limit.is_none());
        assert_eq!(retained.transport_truncation, None);
        let retained_wire = serde_json::to_value(&retained).unwrap();
        assert_eq!(retained_wire, tool_body["output"]);
        for stream in [CommandStream::Stdout, CommandStream::Stderr] {
            assert_eq!(view.read(output_id, stream, 0, 1), Some(Vec::new()));
        }
        for secret in [
            private_path.as_str(),
            harness._root.path().to_str().unwrap(),
            "private-spawn-path-must-not-exist",
            "PRIVATE_AUTH_TOKEN",
            environment_secret,
            command_secret,
            command.as_str(),
            tool_call_secret,
        ] {
            assert!(
                !content.contains(secret),
                "private input leaked into Bash ToolOutput body"
            );
        }
        assert!(!harness._root.path().join("must-not-run").exists());
        harness.remote.close().await.unwrap();
        assert!(harness.session.command_snapshot().is_empty());
        assert_eq!(
            serde_json::to_value(view.metadata(output_id).unwrap()).unwrap(),
            retained_wire
        );
    })
    .await
    .expect("bounded real spawn failure over signed HTTP");
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
        assert_eq!(terminal.process_failure, None);
        assert_eq!(
            terminal.output_collection,
            workdir::CommandOutputCollection::Complete
        );
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
            assert_eq!(terminal.process_failure, None);
            assert_eq!(
                terminal.output_collection,
                workdir::CommandOutputCollection::Complete
            );
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
