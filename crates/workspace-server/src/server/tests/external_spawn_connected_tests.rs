//! Signed Browser create -> Backend reservation -> real Runtime/materializer/Controller
//! -> signed Workdir proxy -> external local session. Only model output and the
//! provider socket transport are substituted; roots, SQLite and authorization are real.
//! Run with the normal Server library `external_spawn_connected` filter.
use super::*;

struct ExternalShare {
    id: String,
    root: tempfile::TempDir,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for ExternalShare {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl ExternalShare {
    fn new(api: &WorkspaceApi, id: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("README.md"), "external clean\n").unwrap();
        let now = Utc::now().to_rfc3339();
        let grant = ExternalWorkdirGrantRecord {
            grant_id: format!("grant-{id}"),
            workspace_id: TEST_WORKSPACE_ID.into(),
            workdir_id: id.into(),
            provider_instance_id: format!("provider-{id}"),
            session_credential_digest: Some(crate::auth::token_hash(&"a".repeat(64))),
            reconnect_deadline_at: Some((Utc::now() + Duration::minutes(5)).to_rfc3339()),
            display_name: id.into(),
            permissions: "read_only".into(),
            created_by: test_browser_request_actor().account_id,
            created_at: now.clone(),
            expires_at: None,
            generation: 1,
            status: "pending".into(),
            updated_at: now.clone(),
        };
        api.store
            .create_external_workdir_grant(
                &grant,
                &WorkdirRegistryRecord {
                    workspace_id: TEST_WORKSPACE_ID.into(),
                    workdir_id: id.into(),
                    display_name: Some(id.into()),
                    source: WorkdirRegistrySource::ExternalGrant {
                        grant_id: grant.grant_id.clone(),
                    },
                    creation_selector: None,
                    creation_ref: None,
                    creation_tree: None,
                    current_selector: None,
                    current_ref: None,
                    current_tree: None,
                    observed_at_epoch_seconds: None,
                    materialization_status: "pending".into(),
                    cleanliness: "unknown".into(),
                    created_at: now.clone(),
                    updated_at: now.clone(),
                },
            )
            .unwrap();
        assert!(
            api.store
                .activate_external_workdir_grant(
                    TEST_WORKSPACE_ID,
                    &grant.grant_id,
                    &grant.provider_instance_id,
                    1,
                    &now
                )
                .unwrap()
        );
        let capabilities = workdir::WorkdirSessionCapabilities::READ_ONLY;
        let (sender, mut receiver) = tokio::sync::mpsc::channel(16);
        api.external_workdir_providers
            .lock()
            .unwrap()
            .connected(Arc::new(ExternalProviderConnection {
                grant_id: grant.grant_id,
                workdir_id: id.into(),
                provider_instance_id: grant.provider_instance_id,
                generation: 1,
                expires_at: None,
                capabilities,
                access_fenced: std::sync::atomic::AtomicBool::new(false),
                admission: Arc::new(tokio::sync::Semaphore::new(16)),
                shutdown_confirmed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                sender,
            }));
        let session: WorkdirSessionHandle = Arc::new(
            workdir::LocalWorkdirSession::external_with_capabilities_pinned(
                workdir::Workdir::new(id),
                workdir::ExternalWorkdirRoot::pin(root.path()).unwrap(),
                workdir::BoundedReadLimits::EXTERNAL_DEFAULT,
                capabilities,
            )
            .unwrap(),
        );
        let task = tokio::spawn(async move {
            while let Some(command) = receiver.recv().await {
                match command {
                    ExternalProviderCommand::Operation {
                        operation,
                        response,
                        ..
                    } => {
                        let result = execute_workdir_session_operation(&session, operation)
                            .await
                            .map_err(workdir_operation_transport_error);
                        let _ = response.send(result);
                    }
                    ExternalProviderCommand::Cancel { .. } => {}
                    ExternalProviderCommand::Revoke { .. } => break,
                }
            }
        });
        Self {
            id: id.into(),
            root,
            task,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn external_spawn_initial_turn_reads_provider_and_replay_keeps_one_worker() {
    assert_external_spawn(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn mixed_external_and_repository_spawn_preserves_provider_capabilities() {
    assert_external_spawn(true).await;
}

async fn assert_external_spawn(mixed: bool) {
    use worker_runtime::working_directory::WorkingDirectoryMaterializer;
    let (completed, mut completion) = tokio::sync::mpsc::channel(4);
    let harness = ConnectedWorkerHarness::start(ConnectedAuthorizationModel { completed }).await;
    let share = ExternalShare::new(&harness.api, "shared-code");
    let reference = mixed.then(|| ExternalShare::new(&harness.api, "shared-reference"));
    assert!(
        harness
            .materializer
            .bind_working_directory(&share.id, None)
            .is_err()
    );
    let mut attachments = vec![json!({"alias":"workdir","working_directory_id":share.id})];
    if let Some(reference) = &reference {
        let (status, repository) = harness.human(reqwest::Method::POST,
            &format!("/api/w/{TEST_WORKSPACE_ID}/working-directories"),
            json!({"runtime_id":CONNECTED_RUNTIME,"repository_key":"test-repository","selector":"HEAD","operation_id":"mixed-checkout"})
        ).await;
        assert_eq!(status, StatusCode::CREATED, "{repository}");
        let checkout = repository["item"]["working_directory_id"].as_str().unwrap();
        attachments.push(json!({"alias":"reference","working_directory_id":reference.id}));
        attachments.push(json!({"alias":"checkout","working_directory_id":checkout}));
    }
    let request = json!({
        "runtime_id": CONNECTED_RUNTIME, "display_name": "External investigation", "profile": "builtin:coder",
        "control_operation_id": "external-create-once", "initial_submit": [{"kind":"text","content":"Read README.md, then attempt Write."}],
        "workdir_attachments": attachments, "feature_connections": {}, "authorization":{"kind":"assignment"}
    });
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/workers");
    let (status, created) = harness
        .human(reqwest::Method::POST, &path, request.clone())
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{created}; {:?}",
        harness.runtime_errors.lock().unwrap()
    );
    let worker = RuntimeWorkerRef::new(CONNECTED_RUNTIME, created["worker_id"].as_str().unwrap());
    let links = harness
        .api
        .store
        .list_worker_workdir_links(TEST_WORKSPACE_ID, &worker)
        .unwrap();
    assert_eq!(links.len(), attachments.len());
    for link in &links {
        assert_eq!(
            link.capabilities,
            if link.alias == "checkout" {
                workdir::WorkdirSessionCapabilities::ALL
            } else {
                workdir::WorkdirSessionCapabilities::READ_ONLY
            }
        );
    }
    let results = tokio::time::timeout(std::time::Duration::from_secs(30), completion.recv())
        .await
        .unwrap()
        .unwrap();
    let read = results
        .iter()
        .find(|(id, _, _)| id == "connected-read")
        .unwrap();
    assert!(!read.1 && read.2.contains("external clean"), "{read:?}");
    let write = results
        .iter()
        .find(|(id, _, _)| id == "connected-write")
        .unwrap();
    assert!(write.1, "provider ceiling must deny Write: {write:?}");
    assert_eq!(
        fs::read_to_string(share.root.path().join("README.md")).unwrap(),
        "external clean\n"
    );
    let (status, replay) = harness.human(reqwest::Method::POST, &path, request).await;
    assert_eq!(status, StatusCode::OK, "{replay}");
    assert_eq!(replay["worker_id"], created["worker_id"]);
    assert_eq!(
        harness
            .api
            .store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, &worker)
            .unwrap(),
        links
    );
    assert!(
        completion.try_recv().is_err(),
        "replay must not dispatch another turn"
    );
    harness
        .api
        .runtime
        .stop_worker(
            &worker,
            WorkerLifecycleRequest {
                stop_intent: None,
                reason: None,
                ticket_assignment: None,
            },
        )
        .unwrap();
}
