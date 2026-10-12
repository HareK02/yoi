use super::*;
use sha2::Digest;
use std::sync::atomic::AtomicI64;
use tokio::sync::{mpsc, oneshot};

const START: i64 = 1_800_000_000;
const BOUND: std::time::Duration = std::time::Duration::from_secs(5);

struct Fixture {
    _root: tempfile::TempDir,
    api: WorkspaceApi,
    server: WorkspaceServerApi,
    app: Router,
    identity: RuntimeIdentityMaterial,
    now: Arc<AtomicI64>,
}

impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut api = test_api(root.path()).await;
        let identity = RuntimeIdentityMaterial::generate("workdirless-runtime").unwrap();
        configure_runtime_request_auth(&mut api, &identity, "workdirless-runtime");
        let runtime = WorkdirlessFixtureRuntime::default();
        for worker in ["1", "2"] {
            seed_worker_source_member(&api, WorkdirlessFixtureRuntime::RUNTIME_ID, worker);
            runtime.workers.lock().unwrap().push(InternalWorkerSummary {
                restore_observation_token: None,
                worker: RuntimeWorkerRef::new(WorkdirlessFixtureRuntime::RUNTIME_ID, worker),
                host_id: "fixture-host".into(),
                display_name: worker.into(),
                label: worker.into(),
                profile: None,
                singleton_key: None,
                tags: Vec::new(),
                workspace: InternalWorkerWorkspaceSummary {
                    visibility: "workspace".into(),
                    identity: TEST_WORKSPACE_ID.into(),
                    workspace_id: Some(TEST_WORKSPACE_ID.into()),
                },
                availability: protocol::subscription::SubscriptionWorkerAvailability::Observed,
                state: "idle".into(),
                execution_reconstructed: false,
                worker_state: None,
                last_seen_at: None,
                pinned: false,
                retention_state: "normal".into(),
                implementation: InternalWorkerImplementationSummary {
                    kind: "fixture".into(),
                    display_hint: "fixture".into(),
                },
                workdir_attachments: Vec::new(),
                diagnostics: Vec::new(),
            });
        }
        api.runtime.register_or_replace(runtime);
        let now = Arc::new(AtomicI64::new(START));
        let clock = now.clone();
        let mut server = WorkspaceServerApi::new(api.config.clone(), api.store.clone());
        server.proof_clock = Arc::new(move || clock.load(Ordering::SeqCst));
        server
            .apis
            .lock()
            .await
            .insert(TEST_WORKSPACE_ID.into(), api.clone());
        server
            .routers
            .lock()
            .await
            .insert(TEST_WORKSPACE_ID.into(), build_inner_router(api.clone()));
        let app = workspace_server_router(server.clone());
        Self {
            _root: root,
            api,
            server,
            app,
            identity,
            now,
        }
    }

    fn request(
        &self,
        worker: Option<&str>,
        path: &str,
        body: Vec<u8>,
        issued: i64,
        permission: &str,
    ) -> Request<Body> {
        let proof = worker_runtime::auth::RuntimeRequestSourceSigner::from_identity(&self.identity)
            .issue(
                "server-test",
                TEST_WORKSPACE_ID,
                worker,
                permission,
                "POST",
                path,
                &body,
                issued,
                60,
            )
            .unwrap();
        Request::builder()
            .method("POST")
            .uri(path)
            .header(CONTENT_TYPE, "application/json")
            .header(
                worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER,
                proof,
            )
            .body(Body::from(body))
            .unwrap()
    }

    fn operation(&self, worker: &str) -> Request<Body> {
        let body = serde_json::to_vec(&WorkspaceWorkdirSessionOperationRequest {
            target_workdir: "attachment".into(),
            operation: WorkdirSessionOperation::Read(workdir::ReadRequest {
                path: workdir::WorkdirPath::new("visible.txt").unwrap(),
                offset: 0,
                limit: 10,
                max_bytes: 1024,
            }),
        })
        .unwrap();
        self.request(
            Some(worker),
            &format!("/api/w/{TEST_WORKSPACE_ID}/workers/self/workdir-session/operations"),
            body,
            self.now.load(Ordering::SeqCst),
            worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
        )
    }

    fn attach(&self, worker: &str) -> mpsc::Receiver<ExternalProviderCommand> {
        let id = format!("external-{worker}");
        let grant = ExternalWorkdirGrantRecord {
            grant_id: id.clone(),
            workspace_id: TEST_WORKSPACE_ID.into(),
            workdir_id: id.clone(),
            provider_instance_id: id.clone(),
            display_name: id.clone(),
            permissions: "read_only".into(),
            created_by: "account-test".into(),
            created_at: "1".into(),
            expires_at: None,
            generation: 1,
            status: "online".into(),
            updated_at: "1".into(),
        };
        self.api
            .store
            .create_external_workdir_grant(
                &grant,
                &WorkdirRegistryRecord {
                    workspace_id: TEST_WORKSPACE_ID.into(),
                    workdir_id: id.clone(),
                    display_name: Some(id.clone()),
                    source: WorkdirRegistrySource::ExternalGrant {
                        grant_id: id.clone(),
                    },
                    creation_selector: None,
                    creation_ref: None,
                    creation_tree: None,
                    current_selector: None,
                    current_ref: None,
                    current_tree: None,
                    observed_at_epoch_seconds: None,
                    materialization_status: "present".into(),
                    cleanliness: "clean".into(),
                    created_at: "1".into(),
                    updated_at: "1".into(),
                },
            )
            .unwrap();
        self.api
            .store
            .attach_worker_workdir(&WorkerWorkdirLinkRecord {
                connection_id: String::new(),
                workspace_id: TEST_WORKSPACE_ID.into(),
                worker: RuntimeWorkerRef::new("workdirless-runtime", worker),
                workdir_id: id.clone(),
                alias: "attachment".into(),
                capabilities: workdir::WorkdirSessionCapabilities::READ_ONLY,
                linked_at: "1".into(),
                unlinked_at: None,
            })
            .unwrap();
        let (sender, receiver) = mpsc::channel(16);
        self.api
            .external_workdir_providers
            .lock()
            .unwrap()
            .connected(Arc::new(ExternalProviderConnection {
                grant_id: id.clone(),
                workdir_id: id.clone(),
                provider_instance_id: id,
                generation: 1,
                expires_at: None,
                capabilities: workdir::WorkdirSessionCapabilities::READ_ONLY,
                admission: Arc::new(tokio::sync::Semaphore::new(16)),
                shutdown_confirmed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                sender,
            }));
        receiver
    }

    async fn start_read(
        &self,
        worker: &str,
        receiver: &mut mpsc::Receiver<ExternalProviderCommand>,
    ) -> (
        tokio::task::JoinHandle<Response>,
        oneshot::Sender<std::result::Result<WorkdirSessionOperationResult, WorkdirTransportError>>,
    ) {
        let request = self.operation(worker);
        let app = self.app.clone();
        let mut task = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
        let command = loop {
            let command = tokio::select! {
            command = receiver.recv() => command.unwrap(),
            result = &mut task => {
                let result = result.unwrap();
                let status = result.status();
                let body = to_bytes(result.into_body(), usize::MAX).await.unwrap();
                panic!("read ended before provider: {status} {body:?}");
            }
            _ = tokio::time::sleep(BOUND) => panic!("read did not reach provider"),
            };
            if matches!(command, ExternalProviderCommand::Cancel { .. }) {
                continue;
            }
            break command;
        };
        let ExternalProviderCommand::Operation {
            operation,
            response,
            ..
        } = command
        else {
            panic!("expected read")
        };
        assert!(
            matches!(operation, WorkdirSessionOperation::Read(_)),
            "{operation:?}"
        );
        (task, response)
    }
}

fn read_result() -> WorkdirSessionOperationResult {
    WorkdirSessionOperationResult::Read(workdir::ReadResult {
        path: workdir::WorkdirPath::new("visible.txt").unwrap(),
        bytes: b"visible\n".to_vec(),
        start_line: 1,
        total_lines: 1,
        content_hash: sha2::Sha256::digest(b"visible\n").into(),
        truncated: false,
    })
}

async fn assert_json(response: Response, status: StatusCode) -> Value {
    let actual = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(actual, status, "{bytes:?}");
    if bytes.is_empty() {
        assert!(!status.is_success(), "successful route must return JSON");
        Value::Null // Existing non-repository dispatcher rejection is status-only.
    } else {
        serde_json::from_slice(&bytes).unwrap()
    }
}

#[tokio::test]
async fn long_worker_operation_does_not_block_other_worker_ticket_posts_or_workdir_read() {
    let f = Fixture::new().await;
    let mut a = f.attach("1");
    let mut b = f.attach("2");
    let ticket = browser_ticket_backend(&f.api)
        .unwrap()
        .create(ticket::NewTicket::new("parallel ticket"))
        .unwrap();
    let (long, response) = f.start_read("1", &mut a).await;
    // A's proof will now be expired, but was already verified before entering
    // its provider. B's freshly issued proof must not wait for A's operation.
    f.now.store(START + 120, Ordering::SeqCst);
    for (path, body) in [
        (
            format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{}/show", ticket.id),
            b"{}".to_vec(),
        ),
        (
            format!("/api/w/{TEST_WORKSPACE_ID}/tickets/query"),
            b"{}".to_vec(),
        ),
    ] {
        let request = f.request(
            Some("2"),
            &path,
            body,
            START + 120,
            worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
        );
        let result = tokio::time::timeout(BOUND, f.app.clone().oneshot(request))
            .await
            .expect("B ticket POST blocked by A")
            .unwrap();
        let body = assert_json(result, StatusCode::OK).await;
        assert!(body.is_object());
    }
    let (read, reply) = f.start_read("2", &mut b).await;
    reply.send(Ok(read_result())).unwrap();
    let body = assert_json(
        tokio::time::timeout(BOUND, read).await.unwrap().unwrap(),
        StatusCode::OK,
    )
    .await;
    assert!(
        serde_json::to_string(&body)
            .unwrap()
            .contains("visible.txt")
    );
    assert!(!long.is_finished(), "A was not controlled by the fixture");
    response.send(Ok(read_result())).unwrap();
    assert_json(long.await.unwrap(), StatusCode::OK).await;
}

#[tokio::test]
async fn same_worker_session_stays_exclusive_but_proof_is_verified_before_waiting() {
    let f = Fixture::new().await;
    let mut a = f.attach("1");
    let (first, reply) = f.start_read("1", &mut a).await;
    let mut second = Box::pin(f.app.clone().oneshot(f.operation("1")));
    // Poll through dispatch/auth until the handler waits on its resource lock.
    assert!(futures::poll!(&mut second).is_pending());
    assert!(
        a.try_recv().is_err(),
        "same session read escaped resource exclusion"
    );
    f.now.store(START + 120, Ordering::SeqCst);
    reply.send(Ok(read_result())).unwrap();
    assert_json(first.await.unwrap(), StatusCode::OK).await;
    assert!(futures::poll!(&mut second).is_pending());
    let Some(ExternalProviderCommand::Operation { response, .. }) =
        tokio::time::timeout(BOUND, a.recv()).await.unwrap()
    else {
        panic!("expected second read")
    };
    response.send(Ok(read_result())).unwrap();
    assert_json(
        tokio::time::timeout(BOUND, second).await.unwrap().unwrap(),
        StatusCode::OK,
    )
    .await;
}

#[tokio::test]
async fn deletion_fences_new_requests_then_drains_without_callback_starvation() {
    let f = Fixture::new().await;
    let mut a = f.attach("1");
    let (long, reply) = f.start_read("1", &mut a).await;
    let gate = f.server.admission_for_workspace(TEST_WORKSPACE_ID).await;
    let mut deletion = Box::pin(gate.lock_deletion());
    assert!(futures::poll!(&mut deletion).is_pending());
    let blocked = f.app.clone().oneshot(f.operation("2")).await.unwrap();
    assert_json(blocked, StatusCode::CONFLICT).await;
    let callback = gate.admit(true).expect("parent may still need callback");
    reply.send(Ok(read_result())).unwrap();
    assert_json(long.await.unwrap(), StatusCode::OK).await;
    // No new callbacks may prolong the drain once all parents have left.
    assert!(gate.admit(true).is_none());
    assert!(futures::poll!(&mut deletion).is_pending());
    drop(callback);
    let deletion = tokio::time::timeout(BOUND, deletion).await.unwrap();
    assert!(gate.admit(false).is_none());
    assert!(gate.admit(true).is_none());
    drop(deletion);
    assert!(gate.admit(false).is_some());
}

#[tokio::test]
async fn cancelled_deletion_reopens_admission_and_cancelled_dispatch_releases_drain() {
    let f = Fixture::new().await;
    let mut a = f.attach("1");
    let (long, _reply) = f.start_read("1", &mut a).await;
    let gate = f.server.admission_for_workspace(TEST_WORKSPACE_ID).await;
    let mut deletion = Box::pin(gate.lock_deletion());
    assert!(futures::poll!(&mut deletion).is_pending());
    drop(deletion);
    assert!(gate.admit(false).is_some());
    long.abort();
    assert!(long.await.unwrap_err().is_cancelled());
    let deletion = tokio::time::timeout(BOUND, gate.lock_deletion())
        .await
        .expect("cancel leaked admission");
    drop(deletion);
    // Retry via the real dispatcher and resource lock, not only the gate.
    let (retry, reply) = f.start_read("1", &mut a).await;
    reply.send(Ok(read_result())).unwrap();
    assert_json(retry.await.unwrap(), StatusCode::OK).await;
}

#[tokio::test]
async fn provider_error_and_disconnect_release_dispatch_admission() {
    for disconnect in [false, true] {
        let f = Fixture::new().await;
        let mut a = f.attach("1");
        let (long, reply) = f.start_read("1", &mut a).await;
        if disconnect {
            drop(reply);
        } else {
            reply
                .send(Err(WorkdirTransportError::from_workdir_error(
                    &workdir::WorkdirError::Denied("fixture denied".into()),
                )))
                .unwrap();
        }
        let result = tokio::time::timeout(BOUND, long).await.unwrap().unwrap();
        assert_json(
            result,
            if disconnect {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::FORBIDDEN
            },
        )
        .await;
        let gate = f.server.admission_for_workspace(TEST_WORKSPACE_ID).await;
        let deletion = tokio::time::timeout(BOUND, gate.lock_deletion())
            .await
            .expect("error leaked admission");
        drop(deletion);
        let (retry, reply) = f.start_read("1", &mut a).await;
        reply.send(Ok(read_result())).unwrap();
        assert_json(retry.await.unwrap(), StatusCode::OK).await;
    }
}

#[tokio::test]
async fn runtime_proof_expiry_tamper_binding_replay_and_membership_are_not_relaxed() {
    let f = Fixture::new().await;
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/query");
    for case in [
        "expired",
        "future",
        "body",
        "path",
        "method",
        "signature",
        "membership",
        "replay",
    ] {
        let issued = match case {
            "expired" => START - 61,
            "future" => START + 61,
            _ => START,
        };
        let mut request = f.request(
            Some(if case == "membership" {
                "not-a-member"
            } else {
                "2"
            }),
            &path,
            b"{}".to_vec(),
            issued,
            worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
        );
        match case {
            "body" => *request.body_mut() = Body::from("{\"limit\":1}"),
            "path" => *request.uri_mut() = format!("{path}?limit=1").parse().unwrap(),
            "method" => *request.method_mut() = Method::GET,
            "signature" => {
                let key = worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER;
                let proof = request.headers()[key].to_str().unwrap().to_owned();
                let mut bytes = proof.into_bytes();
                let index = bytes.iter().position(|b| *b == b'.').unwrap() + 1;
                bytes[index] = if bytes[index] == b'A' { b'B' } else { b'A' };
                request
                    .headers_mut()
                    .insert(key, HeaderValue::from_bytes(&bytes).unwrap());
            }
            "replay" => {
                let proof = request.headers()
                    [worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER]
                    .clone();
                let mut first = f.request(
                    Some("2"),
                    &path,
                    b"{}".to_vec(),
                    START,
                    worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
                );
                first.headers_mut().insert(
                    worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER,
                    proof,
                );
                assert_json(f.app.clone().oneshot(first).await.unwrap(), StatusCode::OK).await;
            }
            _ => {}
        }
        assert_json(
            f.app.clone().oneshot(request).await.unwrap(),
            StatusCode::UNAUTHORIZED,
        )
        .await;
    }
    let gate = f.server.admission_for_workspace(TEST_WORKSPACE_ID).await;
    drop(
        tokio::time::timeout(BOUND, gate.lock_deletion())
            .await
            .unwrap(),
    );
    assert_json(
        f.app
            .clone()
            .oneshot(f.request(
                Some("2"),
                &path,
                b"{}".to_vec(),
                START,
                worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
            ))
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await;
}

#[tokio::test]
async fn provider_timeout_releases_admission_without_wall_clock_wait() {
    let f = Fixture::new().await;
    let mut a = f.attach("1");
    let (long, _reply) = f.start_read("1", &mut a).await;
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(31)).await;
    let body = assert_json(long.await.unwrap(), StatusCode::SERVICE_UNAVAILABLE).await;
    assert_eq!(body["code"], "unavailable");
    tokio::time::resume();
    let gate = f.server.admission_for_workspace(TEST_WORKSPACE_ID).await;
    drop(
        tokio::time::timeout(BOUND, gate.lock_deletion())
            .await
            .unwrap(),
    );
    let (retry, reply) = f.start_read("1", &mut a).await;
    reply.send(Ok(read_result())).unwrap();
    assert_json(retry.await.unwrap(), StatusCode::OK).await;
}

#[tokio::test]
async fn same_workdir_attachment_conflict_is_preserved_while_owner_operation_runs() {
    let f = Fixture::new().await;
    let mut a = f.attach("1");
    let (first, reply) = f.start_read("1", &mut a).await;
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/workers/self/workdir-attachments");
    let request = f.request(
        Some("2"),
        &path,
        serde_json::to_vec(&json!({"alias":"attachment", "working_directory_id":"external-1"}))
            .unwrap(),
        START,
        worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
    );
    let body = assert_json(
        tokio::time::timeout(BOUND, f.app.clone().oneshot(request))
            .await
            .unwrap()
            .unwrap(),
        StatusCode::CONFLICT,
    )
    .await;
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("already attached"),
        "{body}"
    );
    assert!(!first.is_finished());
    reply.send(Ok(read_result())).unwrap();
    assert_json(first.await.unwrap(), StatusCode::OK).await;
}

#[tokio::test]
async fn runtime_callbacks_during_deletion_drain_still_require_bound_proofs() {
    let f = Fixture::new().await;
    let gate = f.server.admission_for_workspace(TEST_WORKSPACE_ID).await;
    let parent = gate.admit(false).unwrap();
    let mut deletion = Box::pin(gate.lock_deletion());
    assert!(futures::poll!(&mut deletion).is_pending());
    let path = format!("/api/runtime/v1/workspaces/{TEST_WORKSPACE_ID}/resources/fetch");
    let handle = f
        .api
        .resource_broker
        .issue_repository_ssh_access_handle(
            TEST_WORKSPACE_ID,
            WorkdirlessFixtureRuntime::RUNTIME_ID,
            "fixture-resource",
            Utc::now().timestamp() + 60,
            worker_runtime::resource::RepositorySshAccessSecret {
                credential_candidates: Vec::new(),
                known_hosts_entry: "fixture".into(),
            },
        )
        .unwrap();
    let body = serde_json::to_vec(&worker_runtime::resource::BackendResourceFetchRequest {
        runtime_id: WorkdirlessFixtureRuntime::RUNTIME_ID.into(),
        worker_id: None,
        audit_correlation_id: handle.audit_correlation_id.clone(),
        handle,
    })
    .unwrap();
    for (permission, expected) in [
        (
            worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
            StatusCode::UNAUTHORIZED,
        ),
        (
            worker_runtime::auth::BACKEND_RESOURCE_FETCH_PERMISSION,
            StatusCode::OK,
        ),
        (
            worker_runtime::auth::BACKEND_RESOURCE_FETCH_PERMISSION,
            StatusCode::NOT_FOUND,
        ),
    ] {
        let request = f.request(None, &path, body.clone(), START, permission);
        let response = tokio::time::timeout(BOUND, f.app.clone().oneshot(request))
            .await
            .unwrap()
            .unwrap();
        assert_json(response, expected).await;
    }
    for (path, permission) in [
        (
            format!("/api/w/{TEST_WORKSPACE_ID}/runtime-config?profile=builtin%3Acompanion"),
            worker_runtime::auth::BACKEND_RESOURCE_FETCH_PERMISSION,
        ),
        (
            format!("/api/w/{TEST_WORKSPACE_ID}/worker-discovery/workers?limit=1"),
            worker_runtime::auth::WORKSPACE_WORKER_DISCOVERY_PERMISSION,
        ),
    ] {
        let proof = worker_runtime::auth::RuntimeRequestSourceSigner::from_identity(&f.identity)
            .issue(
                "server-test",
                TEST_WORKSPACE_ID,
                None,
                permission,
                "GET",
                &path,
                b"",
                START,
                60,
            )
            .unwrap();
        let request = Request::builder()
            .uri(&path)
            .header(
                worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER,
                proof,
            )
            .body(Body::empty())
            .unwrap();
        assert_json(
            tokio::time::timeout(BOUND, f.app.clone().oneshot(request))
                .await
                .unwrap()
                .unwrap(),
            StatusCode::OK,
        )
        .await;
    }
    assert!(futures::poll!(&mut deletion).is_pending());
    drop(parent);
    drop(tokio::time::timeout(BOUND, deletion).await.unwrap());
}

#[tokio::test]
async fn revoked_runtime_and_persisted_workspace_fence_reject_new_requests() {
    for revoked in [true, false] {
        let f = Fixture::new().await;
        if revoked {
            let mut binding = f
                .api
                .store
                .get_workspace_runtime_binding(
                    TEST_WORKSPACE_ID,
                    WorkdirlessFixtureRuntime::RUNTIME_ID,
                )
                .await
                .unwrap()
                .unwrap();
            binding.revoked_at = Some("revoked".into());
            f.api
                .store
                .upsert_workspace_runtime_binding_record(binding, true)
                .await
                .unwrap();
        } else {
            let mut workspace = f
                .api
                .store
                .get_workspace(TEST_WORKSPACE_ID)
                .await
                .unwrap()
                .unwrap();
            workspace.state = "deleting".into();
            f.api.store.upsert_workspace(&workspace).await.unwrap();
        }
        let path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/query");
        let request = f.request(
            Some("2"),
            &path,
            b"{}".to_vec(),
            START,
            worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
        );
        assert_json(
            f.app.clone().oneshot(request).await.unwrap(),
            if revoked {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::CONFLICT
            },
        )
        .await;
        let path = format!("/api/w/{TEST_WORKSPACE_ID}/working-directories");
        let proof = worker_runtime::auth::RuntimeRequestSourceSigner::from_identity(&f.identity)
            .issue(
                "server-test",
                TEST_WORKSPACE_ID,
                Some("2"),
                worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
                "GET",
                &path,
                b"",
                START,
                60,
            )
            .unwrap();
        let request = Request::builder()
            .uri(&path)
            .header(
                worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER,
                proof,
            )
            .body(Body::empty())
            .unwrap();
        assert_json(
            f.app.clone().oneshot(request).await.unwrap(),
            if revoked {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::CONFLICT
            },
        )
        .await;
        drop(
            tokio::time::timeout(
                BOUND,
                f.server
                    .admission_for_workspace(TEST_WORKSPACE_ID)
                    .await
                    .lock_deletion(),
            )
            .await
            .unwrap(),
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_and_restore_session_callbacks_finish_while_deletion_start_waits() {
    let f = Fixture::new().await;
    let subjektiv = open_subjektiv_store(&f.api).unwrap();
    let subject = subjektiv
        .create_subject(crate::subjektiv::SubjectRole::new("companion").unwrap())
        .unwrap();
    let (entered, mut entries) = mpsc::unbounded_channel();
    let (release, released) = std::sync::mpsc::channel();
    let released = Arc::new(Mutex::new(released));
    let runtime = WorkdirlessFixtureRuntime {
        lifecycle_callback: Some(Arc::new(move |worker| {
            entered.send(worker.to_owned()).unwrap();
            released
                .lock()
                .unwrap()
                .recv_timeout(BOUND)
                .expect("callback parent was not released");
        })),
        ..Default::default()
    };
    f.api.runtime.register_or_replace(runtime.clone());
    let token = seed_test_api_token(f.api.store.as_ref(), TEST_WORKSPACE_ID);
    let owner_request = |path: &str, body: Value| {
        Request::builder()
            .method("POST")
            .uri(path)
            .header(CONTENT_TYPE, "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap()
    };
    let mut worker_id = String::new();
    for restore in [false, true] {
        let request = if !restore {
            owner_request(
                &format!("/api/w/{TEST_WORKSPACE_ID}/workers"),
                json!({
                    "runtime_id": WorkdirlessFixtureRuntime::RUNTIME_ID, "display_name": "callback parent",
                    "profile": "builtin:companion", "initial_submit": [],
                    "feature_connections": {"subjektiv":{"subject_id":subject.id}},
                }),
            )
        } else {
            let worker = RuntimeWorkerRef::new(WorkdirlessFixtureRuntime::RUNTIME_ID, &worker_id);
            let observation = f
                .api
                .runtime
                .worker(&worker)
                .unwrap()
                .restore_observation_token
                .unwrap();
            owner_request(
                &format!(
                    "/api/w/{TEST_WORKSPACE_ID}/runtimes/{}/workers/{worker_id}/restore",
                    WorkdirlessFixtureRuntime::RUNTIME_ID
                ),
                json!({"expected_observation_token":observation, "request_id":"restore-callback"}),
            )
        };
        let app = f.app.clone();
        let parent = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
        worker_id = tokio::time::timeout(BOUND, entries.recv())
            .await
            .expect("create/restore did not reach Runtime")
            .unwrap();
        // Use the actual deletion start route, not just the admission helper.
        // Cancel it before preflight: this fixture keeps its Workspace assets.
        let request = owner_request(
            &format!("/api/workspaces/{TEST_WORKSPACE_ID}/deletion"),
            json!({
                "operation_id": if restore {"delete-during-restore"} else {"delete-during-create"},
                "expected_workspace_updated_at": "fixture-updated-at", "confirmation":"fixture",
            }),
        );
        let mut deletion = Box::pin(f.app.clone().oneshot(request));
        assert!(futures::poll!(&mut deletion).is_pending());
        let gate = f.server.admission_for_workspace(TEST_WORKSPACE_ID).await;
        assert!(
            gate.admit(false).is_none(),
            "deletion start did not fence admission"
        );
        let session_id = session_store::new_session_id().to_string();
        let callback_path = format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/sessions");
        let callback = f.request(
            Some(&worker_id),
            &callback_path,
            serde_json::to_vec(&json!({"session_id":session_id,"create_if_missing":true})).unwrap(),
            START,
            worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
        );
        let body = assert_json(
            tokio::time::timeout(BOUND, f.app.clone().oneshot(callback))
                .await
                .unwrap()
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(body["subject_id"], subject.id);
        assert_eq!(
            subjektiv
                .session_attribution(&session_id)
                .unwrap()
                .unwrap()
                .worker_id,
            worker_id
        );
        assert!(!parent.is_finished());
        assert!(futures::poll!(&mut deletion).is_pending());
        drop(deletion);
        release.send(()).unwrap();
        let body = assert_json(
            tokio::time::timeout(BOUND, parent).await.unwrap().unwrap(),
            StatusCode::OK,
        )
        .await;
        if restore {
            assert_eq!(body["result"]["state"], "accepted", "{body}");
        }
        assert!(gate.admit(false).is_some());
        drop(
            tokio::time::timeout(BOUND, gate.lock_deletion())
                .await
                .unwrap(),
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn competing_memory_document_edits_are_atomic_without_workspace_exclusion() {
    use crate::authority::MemoryAuthority;
    let f = Fixture::new().await;
    f.api.authority.update_memory_document("original").unwrap();
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let mut edits = Vec::new();
    for worker in ["1", "2"] {
        let request = f.request(
            Some(worker),
            &format!("/api/w/{TEST_WORKSPACE_ID}/memory/backend"),
            serde_json::to_vec(&MemoryBackendOperation::UpdateDocument(
                memory::backend::MemoryDocumentUpdateOperation {
                    old_string: "original".into(),
                    new_string: format!("winner-{worker}"),
                    replace_all: false,
                },
            ))
            .unwrap(),
            START,
            worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
        );
        let app = f.app.clone();
        let barrier = barrier.clone();
        edits.push(tokio::spawn(async move {
            barrier.wait().await;
            app.oneshot(request).await.unwrap()
        }));
    }
    barrier.wait().await;
    let mut winners = 0;
    let mut rejected = 0;
    for edit in edits {
        let body = assert_json(
            tokio::time::timeout(BOUND, edit).await.unwrap().unwrap(),
            StatusCode::OK,
        )
        .await;
        match body["status"].as_str().unwrap() {
            "ok" => winners += 1,
            "error" => {
                rejected += 1;
                assert!(
                    body["message"].as_str().unwrap().contains("not found"),
                    "{body}"
                );
            }
            _ => panic!("unexpected memory response {body}"),
        }
    }
    assert_eq!((winners, rejected), (1, 1));
    assert!(
        f.api
            .authority
            .memory_document()
            .unwrap()
            .body_md
            .starts_with("winner-")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proof_replay_consumption_is_atomic_across_concurrent_admissions() {
    let f = Fixture::new().await;
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/tickets/query");
    let original = f.request(
        Some("2"),
        &path,
        b"{}".to_vec(),
        START,
        worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
    );
    let proof =
        original.headers()[worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER].clone();
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let mut requests = Vec::new();
    for _ in 0..2 {
        let mut request = f.request(
            Some("2"),
            &path,
            b"{}".to_vec(),
            START,
            worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
        );
        request.headers_mut().insert(
            worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER,
            proof.clone(),
        );
        let app = f.app.clone();
        let barrier = barrier.clone();
        requests.push(tokio::spawn(async move {
            barrier.wait().await;
            app.oneshot(request).await.unwrap()
        }));
    }
    barrier.wait().await;
    let mut statuses = Vec::new();
    for request in requests {
        let response = tokio::time::timeout(BOUND, request).await.unwrap().unwrap();
        let status = response.status();
        assert!(matches!(status, StatusCode::OK | StatusCode::UNAUTHORIZED));
        assert_json(response, status).await;
        statuses.push(status.as_u16());
    }
    statuses.sort();
    assert_eq!(statuses, [200, 401]);
}
