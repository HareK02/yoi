//! Restored Runtime parent client -> installed public Worker tools -> signed loopback
//! HTTP -> real SQLite grants -> live Runtime child -> canonical Session storage.
//! Only the external model is fixed. P is restored without submitting a model turn;
//! C stays live (this does not test child restore). Run with the normal Server lib target.
use super::*;
use agen::tool::{Tool, ToolExecutionContext, ToolOutput};
use futures::FutureExt;
use std::{panic::AssertUnwindSafe, sync::Mutex, time::Duration};
use worker_runtime::worker_backend::{
    ProfileRuntimeWorkerFactory, RuntimeWorkerController, RuntimeWorkerFactory,
    WorkerRuntimeExecutionBackend,
};

pub(super) fn connected_restore_tempdir() -> tempfile::TempDir {
    crate::restore_test_environment::require_restore_test_environment();
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    tempfile::Builder::new()
        .prefix("c")
        .rand_bytes(3)
        .tempdir_in(checkout.join("target"))
        .unwrap()
}

const WATCHDOG: Duration = Duration::from_secs(45);

#[derive(Clone)]
struct FixedModel;
#[async_trait::async_trait]
impl agen::llm_client::LlmClient for FixedModel {
    fn clone_boxed(&self) -> Box<dyn agen::llm_client::LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        _: agen::llm_client::Request,
    ) -> std::result::Result<agen::llm_client::client::ResponseStream, agen::llm_client::ClientError>
    {
        Ok(Box::pin(futures::stream::empty()))
    }
}

#[derive(Default)]
struct Captures {
    handles: Mutex<Vec<(worker_runtime::identity::WorkerRef, worker::WorkerHandle)>>,
    restored_clients: Mutex<
        Vec<(
            worker_runtime::identity::WorkerRef,
            Arc<dyn worker::WorkspaceClient>,
        )>,
    >,
}

// Keep the production factory/restore/signers. The archive chooses an unauthenticated
// model so restoring an idle parent never depends on developer credentials. The spawn
// hook fixes only C's external model boundary; it is deliberately not a restore hook.
struct CapturingFactory {
    inner: ProfileRuntimeWorkerFactory,
    captures: Arc<Captures>,
}
#[async_trait::async_trait]
impl RuntimeWorkerFactory for CapturingFactory {
    async fn spawn_controller(
        &self,
        mut request: worker_runtime::execution::WorkerExecutionSpawnRequest,
    ) -> std::result::Result<RuntimeWorkerController, String> {
        let archive = worker_runtime::profile_archive::ProfileSourceArchive::build(
            worker_runtime::profile_archive::ProfileSourceArchiveInput {
                id: "profile-source-archive:restored-parent".into(),
                entrypoints: BTreeMap::from([("builtin:companion".into(), "profiles/default.dcdl".into())]),
                imports: BTreeMap::new(),
                sources: BTreeMap::from([("profiles/default.dcdl".into(), r#"{
                    slug = "default"; description = "Restored parent connection"; scope = "workspace_read";
                    model = { scheme = "anthropic"; model_id = "test-model"; auth = { kind = "none"; }; };
                    engine = { max_tokens = 100; };
                }"#.into())]),
            },
        ).map_err(|e| e.to_string())?;
        request.request.profile_source =
            worker_runtime::catalog::ProfileSourceArchiveSource::Embedded { archive };
        let worker_ref = request.worker_ref.clone();
        let controller = self.inner.spawn_controller(request).await?;
        self.captures
            .handles
            .lock()
            .unwrap()
            .push((worker_ref, controller.handle.clone()));
        Ok(controller)
    }
    async fn preflight_restore(
        &self,
        request: &worker_runtime::execution::WorkerExecutionRestoreRequest,
    ) -> std::result::Result<(), String> {
        self.inner.preflight_restore(request).await
    }
    async fn recover_restore_source(
        &self,
        request: &worker_runtime::execution::WorkerExecutionRestoreRequest,
    ) -> std::result::Result<bool, worker_runtime::worker_backend::RuntimeRestoreFailure> {
        self.inner.recover_restore_source(request).await
    }

    async fn prepare_restore(
        &self,
        request: worker_runtime::execution::WorkerExecutionRestoreRequest,
    ) -> std::result::Result<
        Box<dyn worker_runtime::worker_backend::RuntimeRestoreCandidate>,
        worker_runtime::worker_backend::RuntimeRestoreFailure,
    > {
        let worker_ref = request.worker_ref.clone();
        let inner = self.inner.prepare_restore(request).await?;
        Ok(Box::new(CapturingCandidate {
            inner,
            worker_ref,
            captures: self.captures.clone(),
        }))
    }

    async fn cleanup_failed_restore(
        &self,
        request: &worker_runtime::execution::WorkerExecutionRestoreRequest,
    ) -> std::result::Result<(), worker_runtime::worker_backend::RuntimeRestoreCleanupFailure> {
        self.inner.cleanup_failed_restore(request).await
    }

    async fn reconcile_stopped_worker(
        &self,
        worker_ref: &worker_runtime::identity::WorkerRef,
    ) -> std::result::Result<(), String> {
        self.inner.reconcile_stopped_worker(worker_ref).await
    }

    fn activate_restored_controller(
        &self,
        worker_ref: &worker_runtime::identity::WorkerRef,
        workspace_id: Option<&str>,
        handle: &worker::WorkerHandle,
    ) -> std::result::Result<(), String> {
        self.inner
            .activate_restored_controller(worker_ref, workspace_id, handle)
    }
}

struct CapturingCandidate {
    inner: Box<dyn worker_runtime::worker_backend::RuntimeRestoreCandidate>,
    worker_ref: worker_runtime::identity::WorkerRef,
    captures: Arc<Captures>,
}

#[async_trait::async_trait]
impl worker_runtime::worker_backend::RuntimeRestoreCandidate for CapturingCandidate {
    fn initial_state(&self) -> protocol::WorkerStateSnapshot {
        self.inner.initial_state()
    }

    async fn activate(
        &mut self,
    ) -> std::result::Result<
        RuntimeWorkerController,
        worker_runtime::worker_backend::RuntimeRestoreFailure,
    > {
        let controller = self.inner.activate().await?;
        self.captures
            .handles
            .lock()
            .unwrap()
            .push((self.worker_ref.clone(), controller.handle.clone()));
        self.captures
            .restored_clients
            .lock()
            .unwrap()
            .push((self.worker_ref.clone(), controller.workspace_client.clone()));
        Ok(controller)
    }

    async fn discard(
        &mut self,
    ) -> std::result::Result<(), worker_runtime::worker_backend::RuntimeRestoreCleanupFailure> {
        self.inner.discard().await
    }
}

struct Resources {
    execution: Arc<WorkerRuntimeExecutionBackend<CapturingFactory>>,
    captures: Arc<Captures>,
    servers: Vec<tokio::task::JoinHandle<()>>,
    api: Option<WorkspaceApi>,
}
impl Resources {
    async fn close(&mut self) {
        for server in self.servers.drain(..) {
            server.abort();
            assert!(server.await.unwrap_err().is_cancelled());
        }
        let execution = self.execution.clone();
        let workers = self
            .captures
            .handles
            .lock()
            .unwrap()
            .iter()
            .map(|(id, _)| id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        // stop_worker waits for shutdown completion and joins all execution-owned tasks.
        tokio::task::spawn_blocking(move || {
            use worker_runtime::execution::WorkerExecutionBackend;
            for worker in workers {
                let result = execution.stop_worker(&worker);
                assert!(result.is_accepted(), "cleanup {worker:?}: {result:?}");
            }
        })
        .await
        .unwrap();
        if let Some(api) = self.api.take() {
            shutdown_workspace_feature_storage(&api).await.unwrap();
        }
    }
}

async fn spawn_pair_member(api: &WorkspaceApi, name: &str) -> RuntimeWorkerRef {
    let Json(created) = create_workspace_worker(
        State(api.clone()),
        HeaderMap::new(),
        Json(CreateWorkspaceWorkerRequest {
            runtime_id: EMBEDDED_RUNTIME_ID.into(),
            display_name: name.into(),
            singleton_key: None,
            profile: Some("builtin:companion".into()),
            ticket_assignment: None,
            initial_submit: vec![],
            workdir_attachments: vec![],
            feature_connections: Default::default(),
            control_operation_id: None,
            execution_grant: None,
        }),
    )
    .await
    .unwrap();
    RuntimeWorkerRef::new(created.runtime_id, created.worker_id)
}

fn handle(captures: &Captures, worker: &RuntimeWorkerRef) -> worker::WorkerHandle {
    captures
        .handles
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|(id, _)| id.worker_id.to_string() == worker.worker_id)
        .unwrap()
        .1
        .clone()
}

async fn finished_submission(events: &mut tokio::sync::broadcast::Receiver<protocol::Event>) {
    let mut accepted_id = None;
    let mut finished = false;
    loop {
        match events
            .recv()
            .await
            .expect("controller event stream closed before Submit completion")
        {
            protocol::Event::SubmissionAccepted {
                submission_request_id,
                ..
            } => {
                assert!(accepted_id.is_none(), "unexpected additional Submit");
                accepted_id = Some(submission_request_id);
            }
            protocol::Event::SubmissionRejected { message, .. } => {
                panic!("Submit rejected: {message}")
            }
            protocol::Event::RunEnd { result } => {
                assert!(accepted_id.is_some());
                assert_eq!(result, protocol::RunResult::Finished);
                finished = true;
            }
            protocol::Event::WorkerState { snapshot }
                if finished
                    && snapshot.state == protocol::WorkerState::Idle
                    && snapshot.last_finished_submission_request_id == accepted_id =>
            {
                return;
            }
            _ => {}
        }
    }
}

// Public Feature installation needs a Worker tool host. This non-controller host
// uses the *restored controller's* client unchanged, not a fabricated parent signer.
async fn tools_for_restored_client(
    root: &Path,
    client: Arc<dyn worker::WorkspaceClient>,
) -> BTreeMap<String, Arc<dyn Tool>> {
    let manifest = worker::WorkerManifest::from_toml(
        "[worker]\nname=\"control-tools\"\n[model]\nscheme=\"anthropic\"\nmodel_id=\"unused\"\n[engine]\n[scope]\nallow=[]\n",
    ).unwrap();
    let engine =
        agen::Engine::<_, agen::state::Mutable, worker::SessionHistoryMetadata>::new_annotated(
            FixedModel,
        );
    let mut host = worker::Worker::new(
        manifest,
        engine,
        session_store::FsStore::new(root.join("tool-host")).unwrap(),
        worker::WorkerWorkspaceContext::with_client(
            Some(worker::WorkspaceId::new(TEST_WORKSPACE_ID).unwrap()),
            client.clone(),
        ),
        worker::WorkerFilesystemAuthority::None,
        manifest::Scope::empty(),
    )
    .await
    .unwrap();
    let report = host.install_features(worker::feature::FeatureRegistryBuilder::new().with_module(
        worker::feature::builtin::manage_worker::manage_worker_feature(client, None, false),
    ));
    assert!(!report.has_errors(), "{}", report.error_message());
    let server = host.engine().tool_server_handle();
    server.flush_pending();
    ["WorkerList", "WorkerSendInput"]
        .into_iter()
        .map(|name| (name.into(), server.get_tool(name).unwrap().1))
        .collect()
}
async fn call(tools: &BTreeMap<String, Arc<dyn Tool>>, name: &str, args: Value) -> ToolOutput {
    tools[name]
        .execute(
            &args.to_string(),
            ToolExecutionContext::new(uuid::Uuid::now_v7().to_string(), "restored-parent", 0),
        )
        .await
        .unwrap_or_else(|error| panic!("{name} from restored parent failed: {error}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restored_runtime_parent_lists_and_inputs_existing_child() {
    let root = connected_restore_tempdir();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let mut config = test_server_config(root.path());
    config.repositories.clear();
    config.backend_base_url = Some(base.clone());
    let factory = ProfileRuntimeWorkerFactory::new(root.path())
        .with_runtime_store_dir(config.embedded_runtime_store_root.clone())
        .with_remote_worker_mutation_identity(EMBEDDED_RUNTIME_REQUEST_IDENTITY.clone())
        .with_workspace_request_client(
            worker_runtime::workspace_request::RuntimeWorkspaceRequestClient::new(
                TEST_WORKSPACE_ID,
                &base,
                EMBEDDED_RUNTIME_ID,
            )
            .with_runtime_request_source(&EMBEDDED_RUNTIME_REQUEST_IDENTITY, &base),
        )
        .with_spawn_model_client_factory(|| Box::new(FixedModel));
    let captures = Arc::new(Captures::default());
    let execution = Arc::new(
        WorkerRuntimeExecutionBackend::new(CapturingFactory {
            inner: factory.clone(),
            captures: captures.clone(),
        })
        .unwrap(),
    );
    let mut resources = Resources {
        execution,
        captures: captures.clone(),
        servers: vec![],
        api: None,
    };
    let exercise = async {
        let store = Arc::new(SqliteWorkspaceStore::open_for_test(&config.database_path).unwrap());
        let old_store = Arc::downgrade(&store);
        let mut api = WorkspaceApi::new_with_execution_backend(
            config.clone(),
            store.clone(),
            resources.execution.clone(),
        )
        .await
        .unwrap();
        configure_runtime_request_auth(
            &mut api,
            &EMBEDDED_RUNTIME_REQUEST_IDENTITY,
            EMBEDDED_RUNTIME_ID,
        );
        api.config.backend_base_url = Some(base.clone());
        let sources = api.config.remote_runtime_sources.clone();
        resources.api = Some(api.clone());
        let server_api = api.clone();
        resources.servers.push(tokio::spawn(async move {
            axum::serve(listener, build_router(server_api))
                .await
                .unwrap()
        }));
        let parent = spawn_pair_member(&api, "Parent").await;
        let child = spawn_pair_member(&api, "Child").await;
        let child_handle = handle(&captures, &child);
        assert_eq!(
            child_handle.shared_state.catalog_status(),
            protocol::WorkerStatus::Idle
        );
        // Materialize P's Session/manifest before stopping, through a real Submit.
        let mut parent_events = handle(&captures, &parent).subscribe();
        let accepted = api
            .runtime
            .send_input(
                &parent,
                WorkerInputRequest {
                    kind: WorkerInputKind::UserIfIdle,
                    content: "parent bootstrap".into(),
                    submission_request_id: None,
                    segments: None,
                },
            )
            .unwrap();
        assert_eq!(accepted.disposition, WorkerInputDisposition::Accepted);
        finished_submission(&mut parent_events).await;
        let grant = WorkerControlGrantRecord {
            workspace_id: TEST_WORKSPACE_ID.into(),
            grant_id: "restored-parent-child".into(),
            controller: parent.clone(),
            subject: child.clone(),
            relation: "spawned".into(),
            origin: "test".into(),
            permissions: vec!["send_input".into(), "observe".into()],
            operation_id: "restored-parent-child".into(),
            created_at: now_registry_timestamp(),
            revoked_at: None,
        };
        api.store.create_worker_control_grant(&grant).unwrap();
        let grants_before = api
            .store
            .list_active_worker_control_grants(TEST_WORKSPACE_ID, &parent, 10)
            .unwrap();
        let stopped = api
            .runtime
            .stop_worker(
                &parent,
                WorkerLifecycleRequest {
                    reason: Some("restart parent".into()),
                    ticket_assignment: None,
                },
            )
            .unwrap();
        assert_eq!(stopped.state, InternalWorkerOperationState::Accepted);
        assert!(
            handle(&captures, &parent).protocol_is_closed(),
            "stop must join the old parent controller"
        );
        let server = resources.servers.pop().unwrap();
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
        shutdown_workspace_feature_storage(&api).await.unwrap();
        resources.api.take();
        drop(api);
        drop(store);
        assert!(
            old_store.upgrade().is_none(),
            "old API/store must be released before reopen"
        );

        // Same DB and Runtime FS. No post-restart registry or grant seed.
        let store = Arc::new(SqliteWorkspaceStore::open_for_test(&config.database_path).unwrap());
        let mut api = WorkspaceApi::new_with_execution_backend(
            config.clone(),
            store,
            resources.execution.clone(),
        )
        .await
        .unwrap();
        api.config.remote_runtime_sources = sources;
        resources.api = Some(api.clone());
        let listener = tokio::net::TcpListener::bind(base.strip_prefix("http://").unwrap())
            .await
            .unwrap();
        let server_api = api.clone();
        resources.servers.push(tokio::spawn(async move {
            axum::serve(listener, build_router(server_api))
                .await
                .unwrap()
        }));
        let observation = api.runtime.worker(&parent).unwrap();
        let restored = api
            .runtime
            .restore_worker(
                &parent,
                runtime_api::WorkerRestoreRequest {
                    expected_observation_token: observation.restore_observation_token.unwrap(),
                    request_id: "restore-parent".into(),
                    preparation: Some(runtime_api::WorkerRestorePreparation::default()),
                },
            )
            .unwrap();
        assert_eq!(
            restored.state,
            server_api::WorkerRestoreState::Accepted,
            "{restored:?}"
        );
        let restored_clients = captures.restored_clients.lock().unwrap().clone();
        assert_eq!(
            restored_clients.len(),
            1,
            "only P is restored; C remains the original live controller"
        );
        assert_eq!(
            restored_clients[0].0.worker_id.to_string(),
            parent.worker_id
        );
        let client = restored_clients[0].1.clone();
        assert_eq!(client.kind(), "runtime-owned-workspace-client");
        let tools = tools_for_restored_client(root.path(), client).await;
        let output = call(&tools, "WorkerList", json!({})).await;
        let list: Value = serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
        let expected_subject = json!({"kind":"runtime_worker", "runtime_id": child.runtime_id, "worker_id": child.worker_id});
        assert_eq!(list["items"].as_array().unwrap().len(), 1, "{list}");
        assert_eq!(
            list["items"][0]["subject"], expected_subject,
            "restored parent's list must expose the complete child subject"
        );
        assert_eq!(list["items"][0]["permissions"], json!(grant.permissions));
        assert_eq!(list["items"][0]["summary"]["runtime_id"], child.runtime_id);
        assert_eq!(list["items"][0]["summary"]["worker_id"], child.worker_id);
        assert_eq!(
            api.store
                .list_active_worker_control_grants(TEST_WORKSPACE_ID, &parent, 10)
                .unwrap(),
            grants_before
        );
        let subject = list["items"][0]["subject"].clone();
        let marker = format!("restored-parent-marker-{}", uuid::Uuid::now_v7());
        let mut child_events = child_handle.subscribe();
        let output = call(
            &tools,
            "WorkerSendInput",
            json!({"subject":subject,"content":marker}),
        )
        .await;
        let input: Value = serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
        assert_eq!(input["state"], "accepted", "{input}");
        finished_submission(&mut child_events).await;
        let child_ref = worker_runtime::identity::WorkerRef::try_from(&child).unwrap();
        let before = factory.retained_session_snapshot(&child_ref).unwrap();
        let bytes = serde_json::to_value(&before.snapshot).unwrap();
        assert_eq!(
            before
                .snapshot
                .entries
                .iter()
                .filter(|entry| matches!(
                    &entry.data,
                    protocol::SessionSnapshotEntryData::UserInput { segments }
                        if segments == &vec![protocol::Segment::text(marker.clone())]
                ))
                .count(),
            1,
            "canonical Session must contain the marker exactly once"
        );
        api.store
            .revoke_worker_control_grant(
                TEST_WORKSPACE_ID,
                &grant.grant_id,
                &now_registry_timestamp(),
            )
            .unwrap();
        let refused = tools["WorkerSendInput"]
            .execute(
                &json!({"subject": subject, "content": marker}).to_string(),
                ToolExecutionContext::new("revoked-input", "restored-parent", 1),
            )
            .await;
        let error = refused.expect_err("revoked grant must reject the next public input");
        assert!(error.to_string().contains("unknown worker"), "{error}");
        let after = factory.retained_session_snapshot(&child_ref).unwrap();
        assert_eq!(
            serde_json::to_value(&after.snapshot).unwrap(),
            bytes,
            "rejected input must not change C's canonical Session"
        );
    };
    let outcome = AssertUnwindSafe(tokio::time::timeout(WATCHDOG, exercise))
        .catch_unwind()
        .await;
    tokio::time::timeout(WATCHDOG, resources.close())
        .await
        .expect("fixture shutdown/join timed out");
    match outcome {
        Ok(result) => result.expect("restored parent connection watchdog expired"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
