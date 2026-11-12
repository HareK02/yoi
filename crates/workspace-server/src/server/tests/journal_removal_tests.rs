//! Server Remove → real Runtime freeze/archive → retained recording handoff and
//! producer release, independently of real Journal Job checkpoint/termination/cleanup.
//! Only the external model is scripted. Historical Sessions use owned Store producers
//! and the normal attribution API; no termination proof or removal receipt is fabricated.
//! Runs in the normal workspace-server lib suite with isolated loopback HTTP and DB/files.
use super::*;
use agen::llm_client::{
    LlmClient, Request,
    error::ClientError,
    event::{Event, StopReason},
    types::Item,
};
use serde_json::{Value, json};
use session_store::{LogEntry, Store as _};
use std::{sync::Mutex, time::Duration};
use tokio::sync::watch;

#[path = "journal_release_tests.rs"]
mod release;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ModelAction {
    Hold,
    Record,
    RecordFirstPrefix,
    PartialThenHold,
    Fail,
}

#[derive(Clone)]
struct RemovalModel {
    action: watch::Sender<ModelAction>,
    inputs: Arc<Mutex<Vec<(String, subjektiv::journal::recording::JournalInput)>>>,
    attempt_inputs: Arc<Mutex<Vec<(String, subjektiv::journal::recording::JournalInput)>>>,
    partial_checkpoint: Arc<tokio::sync::Notify>,
    rejected: Arc<tokio::sync::Notify>,
    changed: Arc<tokio::sync::Notify>,
    source_release: watch::Sender<bool>,
    source_requests: Arc<Mutex<Vec<Request>>>,
    source_changed: Arc<tokio::sync::Notify>,
}
impl RemovalModel {
    fn new() -> Self {
        let (action, _) = watch::channel(ModelAction::Hold);
        let (source_release, _) = watch::channel(false);
        Self {
            action,
            inputs: Arc::default(),
            attempt_inputs: Arc::default(),
            partial_checkpoint: Arc::default(),
            rejected: Arc::default(),
            changed: Arc::default(),
            source_release,
            source_requests: Arc::default(),
            source_changed: Arc::default(),
        }
    }
    fn set(&self, action: ModelAction) {
        self.action.send_replace(action);
    }
    fn respond(
        request: &Request,
        input: &subjektiv::journal::recording::JournalInput,
        partial: bool,
    ) -> Vec<Event> {
        let result = |id: &str| {
            request.items.iter().find_map(|item| match item {
                Item::ToolResult {
                    call_id,
                    is_error,
                    content,
                    ..
                } if call_id == id => {
                    assert!(!is_error, "scripted {id} tool failed (payload omitted)");
                    Some(serde_json::from_str::<Value>(content.as_deref().unwrap()).unwrap())
                }
                _ => None,
            })
        };
        if result("submit").is_some() {
            return vec![
                Event::text_block_start(0),
                Event::text_delta(0, "recorded"),
                Event::text_block_stop(0, Some(StopReason::EndTurn)),
            ];
        }
        // Retry input is immutable, but only its still-unreflected suffix may
        // advance this attempt. Derive it from the actual saved state response.
        let state = result("state");
        let current = state
            .as_ref()
            .map(|state| {
                (
                    state["value"]["current_journal"]["process_order"]
                        .as_u64()
                        .unwrap(),
                    state["value"]["current_journal"]["process_byte"]
                        .as_u64()
                        .unwrap(),
                )
            })
            .unwrap_or((0, 0));
        let mut selected = input
            .selected_ranges
            .iter()
            .filter_map(|range| {
                if range.source.order < current.0
                    || (range.source.order == current.0 && range.bytes.end <= current.1)
                {
                    return None;
                }
                let mut range = range.clone();
                if range.source.order == current.0 {
                    range.bytes.start = range.bytes.start.max(current.1);
                }
                Some(range)
            })
            .collect::<Vec<_>>();
        if partial {
            assert!(selected.len() >= 2);
            selected.truncate(1);
        }
        let generation = result("state")
            .and_then(|state| state["value"]["generation_id"].as_str().map(str::to_owned));
        let (tool, id, value) = if result("checkpoint").is_some() {
            ("SubmitBackendJobResult", "submit", json!({"result":{}}))
        } else if let Some(position) = result("position") {
            (
                "CheckpointJournal",
                "checkpoint",
                json!({
                    "request_id":format!("checkpoint:{}", generation.as_ref().unwrap()), "expected_state":position["value"]["state_token"],
                    "action":{"action":"checkpoint","record_operation_ids":[format!("record:{}", generation.as_ref().unwrap())]},
                    "covered_ranges":selected,
                }),
            )
        } else if let Some(record) = result("record") {
            (
                "UpdateJournalPosition",
                "position",
                json!({
                    "request_id":format!("position:{}", generation.as_ref().unwrap()), "expected_state":record["value"]["state_token"],
                    "action":{"action":"update_position","current_position":"Saved original history; no work outcome inferred"},
                    "covered_ranges":[],
                }),
            )
        } else if let Some(state) = result("state") {
            (
                "ConfirmJournalUnchanged",
                "record",
                json!({
                    "request_id":format!("record:{}", generation.as_ref().unwrap()), "expected_state":state["value"]["current_journal"]["state_token"],
                    "action":{"action":"confirm_unchanged"}, "covered_ranges":selected,
                }),
            )
        } else {
            ("ReadJournalState", "state", json!({}))
        };
        assert!(
            request
                .tools
                .iter()
                .any(|definition| definition.name == tool),
            "missing {tool}"
        );
        vec![
            Event::tool_use_start(0, id, tool),
            Event::tool_input_delta(0, value.to_string()),
            Event::tool_use_stop(0),
        ]
    }
}
#[async_trait::async_trait]
impl LlmClient for RemovalModel {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        request: Request,
    ) -> std::result::Result<
        std::pin::Pin<
            Box<dyn futures::Stream<Item = std::result::Result<Event, ClientError>> + Send>,
        >,
        ClientError,
    > {
        let events = if request
            .tools
            .iter()
            .any(|tool| tool.name == "ReadJournalState")
        {
            let text = request
                .items
                .iter()
                .filter_map(Item::as_text)
                .find_map(|text| {
                    text.lines()
                        .find_map(|line| line.strip_prefix("input_json: "))
                })
                .unwrap();
            let input = subjektiv::journal::recording::JournalInput::decode(
                &serde_json::from_str(text).unwrap(),
            )
            .unwrap();
            let job = request
                .items
                .iter()
                .filter_map(Item::as_text)
                .find_map(|text| text.lines().find_map(|line| line.strip_prefix("job_id: ")))
                .unwrap()
                .to_owned();
            let first_prefix = {
                let mut inputs = self.inputs.lock().unwrap();
                if !inputs.iter().any(|(id, _)| *id == job) {
                    inputs.push((job.clone(), input.clone()));
                }
                inputs.first().is_some_and(|(id, _)| *id == job)
            };
            let attempt = request
                .items
                .iter()
                .filter_map(Item::as_text)
                .find_map(|text| {
                    text.lines()
                        .find_map(|line| line.strip_prefix("attempt_id: "))
                })
                .unwrap()
                .to_owned();
            {
                let mut inputs = self.attempt_inputs.lock().unwrap();
                if !inputs.iter().any(|(id, _)| *id == attempt) {
                    inputs.push((attempt, input.clone()));
                }
            }
            self.changed.notify_waiters();
            let mut action = self.action.subscribe();
            let checkpoint_returned = request.items.iter().any(|item|
                matches!(item, Item::ToolResult { call_id, is_error: false, .. } if call_id == "checkpoint"));
            loop {
                let current = *action.borrow_and_update();
                if current == ModelAction::PartialThenHold && checkpoint_returned {
                    self.partial_checkpoint.notify_one();
                } else if current != ModelAction::Hold
                    && (current != ModelAction::RecordFirstPrefix || first_prefix)
                {
                    break;
                }
                action.changed().await.unwrap();
            }
            if *action.borrow() == ModelAction::Fail {
                self.rejected.notify_one();
                return Err(ClientError::Api {
                    status: Some(400),
                    code: Some("removal_fixture_rejected".into()),
                    message: "scripted external model rejection".into(),
                    retry_after: None,
                });
            }
            let partial = *action.borrow() == ModelAction::PartialThenHold;
            Self::respond(&request, &input, partial)
        } else {
            // Original turns are genuinely pending at the external model boundary.
            // Memory Jobs keep their production startup/completion path unchanged.
            if !request.tools.iter().any(|tool| {
                tool.name == "SubmitBackendJobResult" || tool.name == "SubmitMemorySurface"
            }) {
                self.source_requests.lock().unwrap().push(request.clone());
                self.source_changed.notify_waiters();
                let mut release = self.source_release.subscribe();
                while !*release.borrow_and_update() {
                    release.changed().await.unwrap();
                }
            }
            let submitted = request.items.iter().any(|item| {
                matches!(item,
                Item::ToolResult { call_id, is_error: false, .. } if call_id == "memory")
            });
            if submitted
                || !request.tools.iter().any(|tool| {
                    tool.name == "SubmitBackendJobResult" || tool.name == "SubmitMemorySurface"
                })
            {
                vec![
                    Event::text_block_start(0),
                    Event::text_delta(0, "idle"),
                    Event::text_block_stop(0, Some(StopReason::EndTurn)),
                ]
            } else {
                let (tool, value) = if request
                    .tools
                    .iter()
                    .any(|tool| tool.name == "SubmitMemorySurface")
                {
                    ("SubmitMemorySurface", json!({"points":[]}))
                } else {
                    let input: Value = request
                        .items
                        .iter()
                        .filter_map(Item::as_text)
                        .find_map(|text| {
                            text.lines()
                                .find_map(|line| line.strip_prefix("input_json: "))
                        })
                        .map(|text| serde_json::from_str(text).unwrap())
                        .unwrap();
                    (
                        "SubmitBackendJobResult",
                        json!({"result":{"subject_id":input["subject_id"],"candidate_ids":[]}}),
                    )
                };
                vec![
                    Event::tool_use_start(0, "memory", tool),
                    Event::tool_input_delta(0, value.to_string()),
                    Event::tool_use_stop(0),
                ]
            }
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}

// Join the HTTP service and its accepted connections on a normal fixture restart.
// Aborting only Serve's accept loop can leave Router/API clones in connection tasks.
struct RemovalServer {
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}
impl RemovalServer {
    async fn shutdown(mut self) {
        let _ = self.shutdown.take().unwrap().send(());
        tokio::time::timeout(Duration::from_secs(10), &mut self.task)
            .await
            .expect("fixture HTTP connections must drain before Runtime reopen")
            .expect("fixture HTTP server joins cleanly");
    }
}
impl Drop for RemovalServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        // Failure-path cleanup only; never used as Journal termination evidence.
        self.task.abort();
    }
}

struct RemovalFixture {
    _dir: tempfile::TempDir,
    api: Option<WorkspaceApi>,
    server: Option<RemovalServer>,
    model: RemovalModel,
    source: RuntimeWorkerRef,
    attr: crate::subjektiv::SubjectSessionAttribution,
}
impl Drop for RemovalFixture {
    fn drop(&mut self) {
        self.model.source_release.send_replace(true);
        drop(self.server.take());
    }
}
impl RemovalFixture {
    fn api(&self) -> &WorkspaceApi {
        self.api.as_ref().expect("fixture API is open")
    }
    async fn open(
        config: ServerConfig,
        listener: tokio::net::TcpListener,
        model: RemovalModel,
    ) -> (WorkspaceApi, RemovalServer) {
        let control = Arc::new(test_control_store(&config));
        seed_test_registered_workspace(control.as_ref(), &config)
            .await
            .unwrap();
        let broker = BackendResourceBroker::default();
        let dispatcher = Arc::new(
            crate::worker_source::EmbeddedServerWorkerMutationDispatcher::new(
                config.clone(),
                control.clone(),
            ),
        );
        let audience = embedded_runtime_request_audience(&config).unwrap();
        let factory_model = model.clone();
        let execution = worker_runtime::worker_backend::WorkerRuntimeExecutionBackend::new(
            worker_runtime::worker_backend::ProfileRuntimeWorkerFactory::new(
                config.workspace_execution_root.clone(),
            )
            .with_embedded_worker_mutation_dispatcher(
                EMBEDDED_WORKER_RUNTIME_ID,
                dispatcher.clone(),
            )
            .with_workspace_request_client(
                worker_runtime::workspace_request::RuntimeWorkspaceRequestClient::new(
                    TEST_WORKSPACE_ID,
                    audience.clone(),
                    EMBEDDED_WORKER_RUNTIME_ID,
                )
                .with_runtime_request_source(&EMBEDDED_RUNTIME_REQUEST_IDENTITY, audience),
            )
            .with_runtime_store_dir(config.embedded_runtime_store_root.clone())
            .with_resource_client(Arc::new(broker.clone()))
            .with_spawn_model_client_factory(move || Box::new(factory_model.clone())),
        )
        .unwrap();
        let api = WorkspaceApi::new_with_execution_backend_and_broker(
            config,
            control,
            Arc::new(execution),
            broker,
            HostExecutors::default(),
            Some(dispatcher),
        )
        .await
        .unwrap();
        seed_test_delivery_binding(&api);
        let app = build_router(api.clone());
        let (shutdown, stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = stopped.await;
                })
                .await
                .unwrap()
        });
        (
            api,
            RemovalServer {
                shutdown: Some(shutdown),
                task,
            },
        )
    }
    async fn new() -> Self {
        Self::with_live_source(false).await
    }
    async fn with_live_source(live: bool) -> Self {
        let dir = super::super::restored_parent_control_tests::connected_restore_tempdir();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut config = test_server_config(dir.path());
        config.repositories.clear();
        config.backend_base_url = Some(format!("http://{}", listener.local_addr().unwrap()));
        let model = RemovalModel::new();
        let (api, server) = Self::open(config, listener, model.clone()).await;
        let store = open_subjektiv_store(&api).unwrap();
        let subject = create_journal_subject_as_owner(&api, "remove-full-history").await;
        let source = Self::create_body(&api, &subject.id, live).await;
        if !live {
            Self::stop_body(&api, &source).await;
        }
        let attr = store
            .subject_session_attributions(&subject.id)
            .unwrap()
            .into_iter()
            .find(|attr| attr.worker_id == source.worker_id)
            .unwrap();
        Self {
            _dir: dir,
            api: Some(api),
            server: Some(server),
            model,
            source,
            attr,
        }
    }
    async fn create_body(api: &WorkspaceApi, subject_id: &str, live: bool) -> RuntimeWorkerRef {
        let Json(created) = create_workspace_worker_as_owner(
            State(api.clone()),
            HeaderMap::new(),
            Json(CreateWorkspaceWorkerRequest {
                runtime_id: EMBEDDED_WORKER_RUNTIME_ID.into(),
                display_name: "saved-history-source".into(),
                singleton_key: None,
                profile: Some("builtin:companion".into()),
                ticket_assignment: None,
                initial_submit: if live {
                    vec![protocol::Segment::Text {
                        content: "pending-original-model-turn".into(),
                    }]
                } else {
                    vec![]
                },
                workdir_attachments: vec![],
                feature_connections: server_api::WorkspaceWorkerFeatureConnectionsRequest {
                    subjektiv: Some(server_api::SubjektivWorkerConnectionRequest {
                        subject_id: subject_id.into(),
                    }),
                },
                control_operation_id: Some(uuid::Uuid::now_v7().to_string()),
                authorization: server_api::launch_policy::WorkerCreationAuthorization::Assignment {
                    assignment: None,
                },
            }),
        )
        .await
        .unwrap();
        RuntimeWorkerRef::new(created.runtime_id, created.worker_id)
    }
    async fn stop_body(api: &WorkspaceApi, source: &RuntimeWorkerRef) {
        let runtime = api.runtime.clone();
        let stopped = source.clone();
        tokio::task::spawn_blocking(move || {
            runtime.stop_worker(&stopped, WorkerLifecycleRequest::default())
        })
        .await
        .unwrap()
        .unwrap();
        let summary = api.runtime.worker(source).unwrap();
        assert_eq!(summary.state, "stopped");
        sync_worker_observation(api, &summary).unwrap();
    }
    fn root(&self) -> PathBuf {
        self.api()
            .config
            .embedded_runtime_store_root
            .join("workers")
            .join(&self.source.worker_id)
            .join("session")
    }
    fn owner(&self) -> session_store::worker_sources::WorkerSessionSources {
        session_store::worker_sources::WorkerSessionSources::new(
            self.api().config.embedded_runtime_store_root.clone(),
            session_store::worker_sources::WorkerSessionOwner {
                workspace_id: TEST_WORKSPACE_ID.into(),
                worker_id: self.source.worker_id.clone(),
            },
        )
        .unwrap()
    }
    fn history(id: &str, text: &str) -> session_store::LoggedHistoryEntry {
        session_store::LoggedHistoryEntry {
            item: message(text),
            metadata: session_store::LoggedSessionHistoryMetadata {
                input_metadata: None,
                entry_id: session_store::LoggedSessionHistoryEntryId(id.into()),
                origin: session_store::LoggedSessionHistoryOrigin::LocalHumanInput,
                derivation: None,
            },
        }
    }
    fn start(
        session: session_store::SessionId,
        history: Vec<session_store::LoggedHistoryEntry>,
    ) -> LogEntry {
        LogEntry::AnnotatedSegmentStart {
            ts: 1,
            session_id: session,
            system_prompt: None,
            config: agen::llm_client::RequestConfig::default(),
            history,
            forked_from: None,
            compacted_from: None,
        }
    }
    fn seed_active(&self, history: Vec<session_store::LoggedHistoryEntry>) {
        use session_store::WorkerMetadataStore as _;
        let session = self.attr.session_id.parse().unwrap();
        let segment = Uuid::new_v4();
        let store = session_store::WorkerSessionStore::new(self.root())
            .unwrap()
            .with_runtime_owner(self.owner())
            .unwrap();
        store
            .create_segment(session, segment, &[Self::start(session, history)])
            .unwrap();
        let name = format!("worker-runtime-{}", self.source.worker_id);
        session_store::WorkerAggregateStore::new(self.root().parent().unwrap(), &name)
            .unwrap()
            .write(
                &session_store::WorkerMetadata::new(
                    &name,
                    Some(session_store::WorkerActiveSegmentRef::active_segment(
                        session, segment,
                    )),
                )
                .with_workspace_id(TEST_WORKSPACE_ID),
            )
            .unwrap();
    }
    fn seed_retained(
        &self,
        history: Vec<session_store::LoggedHistoryEntry>,
    ) -> crate::subjektiv::SubjectSessionAttribution {
        let session = session_store::new_session_id();
        let segment = Uuid::new_v4();
        let retention = session_store::retention::SessionRetentionStore::new(
            self.api()
                .config
                .embedded_runtime_store_root
                .join("retention/session-sources"),
        );
        let store = session_store::FsStore::new_runtime_owned(self.owner(), retention).unwrap();
        let mut original = vec![
            Self::start(session, vec![history[0].clone()]),
            LogEntry::TurnEnd {
                ts: 2,
                turn_count: 1,
            },
        ];
        original.extend(history.iter().skip(1).cloned().map(|entry| {
            LogEntry::AnnotatedUserInput {
                ts: 3,
                segments: vec![],
                extensions: vec![],
                history: vec![entry],
            }
        }));
        store.create_segment(session, segment, &original).unwrap();
        // Fork the seed prefix; a new original is appended, not forged into the
        // inherited seed (which must exactly match its parent projection).
        let second = Uuid::new_v4();
        let mut fork = Self::start(session, vec![history[0].clone()]);
        if let LogEntry::AnnotatedSegmentStart { forked_from, .. } = &mut fork {
            *forked_from = Some(session_store::SegmentOrigin {
                segment_id: segment,
                at_turn_index: 0,
            });
        }
        store
            .create_segment(
                session,
                second,
                &[
                    fork,
                    LogEntry::AnnotatedUserInput {
                        ts: 4,
                        segments: vec![],
                        extensions: vec![],
                        history: vec![Self::history(
                            "retained-second-segment",
                            "detached retained original",
                        )],
                    },
                ],
            )
            .unwrap();
        store
            .select_recording_active_segment(session, second)
            .unwrap();
        let guard = store.retention().lock_live(session).unwrap();
        session_store::capture::recording_union_catalog_fs_guarded(
            &self.owner().retained_root(),
            session,
            second,
            Default::default(),
            &guard,
        )
        .expect("retained producer fixture has a complete committed lineage");
        drop(guard);
        let attr = crate::subjektiv::SubjectSessionAttribution::new(
            &self.attr.subject_id,
            &self.source.runtime_id,
            &self.source.worker_id,
            session.to_string(),
        )
        .unwrap();
        subjektiv_journal::bind_host_session(self.api(), attr.clone()).unwrap();
        open_subjektiv_store(self.api())
            .unwrap()
            .session_attribution(&attr.session_id)
            .unwrap()
            .unwrap()
    }
    async fn remove(&self) -> worker::WorkspaceResponse {
        WorkerRemovalService::new(self.api())
            .execute_target_removal(
                self.api().runtime.as_ref(),
                &self.source,
                "preserve all saved originals and hand off recording before removal",
                None,
            )
            .await
            .unwrap()
    }
    fn prepared(&self) -> crate::retention::PreparedWorkerRemoval {
        self.api()
            .config_store
            .recover_worker_removal_execution(TEST_WORKSPACE_ID, &self.source)
            .unwrap()
            .unwrap()
    }
    fn assert_held(&self) {
        let store = open_subjektiv_store(self.api()).unwrap();
        assert!(
            !store
                .journal_worker_was_released(
                    &self.attr.subject_id,
                    &self.source.runtime_id,
                    &self.source.worker_id
                )
                .unwrap()
        );
        let key = crate::subjektiv::subject_worker_singleton_key(&self.attr.subject_id).unwrap();
        assert_eq!(
            self.api()
                .store
                .current_worker_singleton_owner(TEST_WORKSPACE_ID, &key)
                .unwrap()
                .unwrap()
                .worker,
            self.source
        );
        assert!(self.api().runtime.worker(&self.source).is_ok());
        assert!(self.root().exists());
        assert!(
            self.api()
                .config_store
                .worker_session_archive(TEST_WORKSPACE_ID, &self.source, &self.attr.session_id)
                .unwrap()
                .is_none()
        );
    }
    /// Stop at the real archive/handoff boundary, before physical source removal.
    /// Fault tests can then target the later retention/catalog commit independently.
    fn prepare_handoff(&self) -> crate::retention::PreparedWorkerRemoval {
        let inventory = self
            .api()
            .runtime
            .worker_retention_inventory(&self.source)
            .unwrap();
        let plan = self
            .api()
            .config_store
            .plan_worker_removal(
                &crate::retention::WorkerRemovalPlanRequest {
                    workspace_id: TEST_WORKSPACE_ID.into(),
                    worker: self.source.clone(),
                    reason: "archive handoff before injected removal failure".into(),
                    journal_erasure: None,
                },
                &inventory,
            )
            .unwrap();
        let prepared = self
            .api()
            .config_store
            .prepare_worker_removal_execution(
                TEST_WORKSPACE_ID,
                &plan.plan_id,
                &plan.input_fingerprint,
            )
            .unwrap();
        assert!(
            subjektiv_journal::advance_worker_removal(
                &WorkerRemovalService::new(self.api()),
                self.api().runtime.as_ref(),
                &self.source,
                &prepared,
            )
            .unwrap()
            .is_none()
        );
        assert!(self.root().exists(), "publication is not physical removal");
        let progress = self
            .api()
            .config_store
            .get_journal_removal(&prepared)
            .unwrap()
            .unwrap();
        assert_eq!(
            progress.metadata.phase,
            crate::store::JournalRemovalPhase::Released
        );
        assert!(
            !progress
                .metadata
                .retained_recording
                .as_ref()
                .unwrap()
                .completed
        );
        prepared
    }

    fn assert_removed_with_pending_recording(&self) {
        assert_retained_recording(self.api(), &self.source, &self.attr);
        assert!(self.api().runtime.worker(&self.source).is_err());
        assert!(!self.root().exists());
    }

    async fn wait_for_input(&self) {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let changed = self.model.changed.notified();
                if !self.model.inputs.lock().unwrap().is_empty() {
                    break;
                }
                changed.await;
            }
        })
        .await
        .expect("real Journal Controller reached external model");
    }
    async fn wait_removed(&self) {
        // Source removal waits for preservation and its catalog commit only.
        // Journal completion and its dedicated Worker teardown use wait_recorded.
        tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                if self.api()
                    .store
                    .get_worker_registry(TEST_WORKSPACE_ID, &self.source)
                    .unwrap()
                    .is_none()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            let progress = self.api().config_store.get_journal_removal(&self.prepared()).unwrap();
            let mut sessions = vec![self.attr.session_id.clone()];
            if let Some(members) = progress.as_ref().and_then(|progress| progress.metadata.members.as_ref()) {
                sessions.extend(members.iter().map(|member| member.session_id.clone()));
            }
            sessions.sort();
            sessions.dedup();
            let jobs = sessions.iter().flat_map(|session| self.api()
                .store
                .list_backend_journal_jobs(TEST_WORKSPACE_ID, session, None, 100)
                .unwrap()).collect::<Vec<_>>();
            // Never Debug a Job record: its immutable request contains source bodies.
            let states = jobs.iter().map(|job| (
                &job.request.job_id,
                &job.state,
                job.current_attempt,
                &job.failure_category,
                &job.failure_detail,
                &job.input_disposition,
            )).collect::<Vec<_>>();
            let attempts = jobs.iter().map(|job| {
                self.api().store.get_backend_job_attempt(
                    TEST_WORKSPACE_ID,
                    &job.request.job_id,
                    &crate::backend_job::attempt_id(&job.request.job_id, job.current_attempt),
                ).unwrap().map(|attempt| (
                    attempt.attempt_id,
                    attempt.state,
                    attempt.worker,
                    attempt.failure_category,
                    attempt.worker_cleanup_state,
                    attempt.worker_cleanup_failure_category,
                ))
            }).collect::<Vec<_>>();
            panic!(
                "autonomous removal stalled: plan={:?}; progress={:?}; job_states={states:?}; attempts={attempts:?}",
                self.prepared(),
                progress.as_ref().map(|progress| (&progress.metadata.phase, &progress.metadata.cause)),
            )
        });
        assert!(self.api().runtime.worker(&self.source).is_err());
        assert!(!self.root().exists());
    }
    async fn wait_recorded(&self) {
        // Multi-member fixtures chain several bounded Jobs. Each must finish
        // Controller teardown before the next prefix can acquire ownership.
        tokio::time::timeout(Duration::from_secs(240), async {
            loop {
                if self
                    .api()
                    .store
                    .list_retained_journal_recordings(TEST_WORKSPACE_ID, None, 100)
                    .unwrap()
                    .is_empty()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            let progress = self
                .api()
                .store
                .list_retained_journal_recordings(TEST_WORKSPACE_ID, None, 100)
                .unwrap();
            let states = progress
                .iter()
                .flat_map(|p| p.metadata.members.iter().flatten())
                .flat_map(|m| {
                    self.api()
                        .store
                        .list_backend_journal_jobs(TEST_WORKSPACE_ID, &m.session_id, None, 100)
                        .unwrap()
                })
                .map(|job| {
                    let attempts = self
                        .api()
                        .store
                        .list_backend_job_attempts(TEST_WORKSPACE_ID, &job.request.job_id)
                        .unwrap();
                    (
                        job.request.job_id,
                        job.state,
                        job.failure_category,
                        job.failure_detail,
                        attempts
                            .into_iter()
                            .map(|a| {
                                (
                                    a.state,
                                    a.worker_cleanup_state,
                                    a.worker_cleanup_failure_category,
                                    a.worker_cleanup_failure_detail,
                                )
                            })
                            .collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>();
            panic!(
                "retained recording stalled: progress={:?}, jobs={states:?}",
                progress
                    .iter()
                    .map(|p| (&p.metadata.phase, &p.metadata.retained_recording))
                    .collect::<Vec<_>>()
            );
        });
        let progress = self
            .api()
            .config_store
            .get_journal_removal(&self.prepared())
            .unwrap()
            .unwrap();
        assert!(
            progress
                .metadata
                .retained_recording
                .as_ref()
                .unwrap()
                .completed
        );
        for member in progress.metadata.members.as_ref().unwrap() {
            let jobs = self
                .api()
                .store
                .list_backend_journal_jobs(TEST_WORKSPACE_ID, &member.session_id, None, 100)
                .unwrap();
            assert!(
                !jobs.is_empty(),
                "materialized fixture must have real recording Jobs"
            );
            for job in jobs {
                assert_eq!(job.state, BackendJobState::Completed);
                assert_job_teardown(self.api(), &job);
            }
        }
    }
    async fn restart(&mut self) {
        // Reopen at a persisted boundary, with no live original Controller. This is
        // logical Server/Runtime restart, not a claim of arbitrary process-crash safety.
        let config = self.api().config.clone();
        shutdown_workspace_feature_storage(self.api())
            .await
            .unwrap();
        if let Some(server) = self.server.take() {
            server.shutdown().await;
        }
        // The old API owns the Runtime's exclusive filesystem lock. Closing feature
        // storage alone does not release it; actually drop it before opening another.
        drop(self.api.take().unwrap());
        let listener = tokio::net::TcpListener::bind(
            config
                .backend_base_url
                .as_ref()
                .unwrap()
                .strip_prefix("http://")
                .unwrap(),
        )
        .await
        .unwrap();
        let (api, server) = Self::open(config, listener, self.model.clone()).await;
        self.api = Some(api);
        self.server = Some(server);
    }
}

/// Removal commits the source's release, not Journal success or explicit erasure.
/// Check both durable responsibility and actual body readability after registry loss.
fn assert_retained_recording(
    api: &WorkspaceApi,
    source: &RuntimeWorkerRef,
    attr: &crate::subjektiv::SubjectSessionAttribution,
) {
    let prepared = api
        .config_store
        .recover_worker_removal_execution(TEST_WORKSPACE_ID, source)
        .unwrap()
        .unwrap();
    assert_eq!(
        prepared.plan.state,
        crate::retention::WorkerRemovalPlanState::Succeeded
    );
    let progress = api
        .config_store
        .get_journal_removal(&prepared)
        .unwrap()
        .unwrap();
    assert_eq!(
        progress.metadata.phase,
        crate::store::JournalRemovalPhase::Completed
    );
    let recording = progress.metadata.retained_recording.as_ref().unwrap();
    assert!(!recording.completed);
    assert!(
        api.store
            .list_retained_journal_recordings(TEST_WORKSPACE_ID, None, 100)
            .unwrap()
            .iter()
            .any(|p| p.operation_id == progress.operation_id)
    );
    assert!(
        api.store
            .get_worker_registry(TEST_WORKSPACE_ID, source)
            .unwrap()
            .is_none()
    );
    let key = crate::subjektiv::subject_worker_singleton_key(&attr.subject_id).unwrap();
    assert!(
        api.store
            .current_worker_singleton_owner(TEST_WORKSPACE_ID, &key)
            .unwrap()
            .is_none()
    );
    let store = open_subjektiv_store(api).unwrap();
    assert!(
        store
            .journal_worker_was_released(&attr.subject_id, &source.runtime_id, &source.worker_id)
            .unwrap()
    );
    assert_eq!(
        store
            .session_attribution(&attr.session_id)
            .unwrap()
            .unwrap(),
        *attr
    );
    let host = JournalHostContext::validated_session(&store, attr.clone()).unwrap();
    assert!(!store.journal_session_erased(&host).unwrap());
    let archive = api
        .config_store
        .worker_session_archive(TEST_WORKSPACE_ID, source, &attr.session_id)
        .unwrap()
        .unwrap();
    assert_eq!(archive.operation_id, prepared.plan.operation_id);
    assert_eq!(archive.archive_id, recording.archive_id);
    let members = progress.metadata.members.as_ref().unwrap();
    assert!(!members.is_empty());
    for member in members {
        // Read every original, including bytes not yet selected into any Job.
        for descriptor in &member.descriptors {
            let mut bytes = Vec::new();
            while (bytes.len() as u64) < descriptor.total_bytes {
                let slice = api
                    .runtime
                    .capture_worker_removal_session(
                        source,
                        runtime_api::WorkerRemovalSessionCaptureRequest {
                            workspace_id: TEST_WORKSPACE_ID.into(),
                            operation_id: prepared.plan.operation_id.clone(),
                            session_id: member.session_id.parse().unwrap(),
                            active_segment_id: member.active_segment_id,
                            start: session_store::capture::RecordingPosition {
                                canonical_order: descriptor.canonical_order,
                                byte_offset: bytes.len() as u64,
                            },
                            max_entries: 1,
                            max_serialized_bytes:
                                subjektiv::journal::recording::JournalInput::capture_budget(),
                        },
                    )
                    .unwrap();
                let entry = slice
                    .capture
                    .entries
                    .first()
                    .expect("retained original remains readable");
                let session_store::capture::CaptureBody::CanonicalPublic {
                    descriptor: actual,
                    byte_offset,
                    text,
                    ..
                } = &entry.body
                else {
                    panic!("retained original must be canonical public material")
                };
                assert_eq!(actual, descriptor);
                assert_eq!(*byte_offset, bytes.len() as u64);
                assert!(!text.is_empty());
                bytes.extend_from_slice(text.as_bytes());
            }
            assert_eq!(bytes.len() as u64, descriptor.total_bytes);
            assert_eq!(
                Sha256::digest(&bytes)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>(),
                descriptor.sha256,
                "retained bytes must match the frozen original"
            );
        }
    }
}

/// Inspect persisted ownership without minting a recording context for an ended
/// Job or borrowing the released producer's writer authority. This is observation
/// only; assert_job_teardown separately requires actual execution/cleanup evidence.
fn assert_no_active_journal_generation(api: &WorkspaceApi, journal_id: &str) {
    let identity = api
        .feature_storage
        .existing_identity(&api.subjektiv_registration)
        .unwrap();
    let database = api
        .feature_storage
        .open_existing(&api.subjektiv_registration, &identity)
        .unwrap();
    let active = database
        .try_with_connection(|conn| {
            Ok::<bool, feature_storage::FeatureStorageError>(conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM journal_generations WHERE journal_id=?1 AND active=1)",
                [journal_id],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert!(!active, "ended Job must relinquish Journal write ownership");
}

fn assert_job_teardown(api: &WorkspaceApi, job: &crate::backend_job::BackendJobRecord) {
    subjektiv_journal::require_job_effects_ended(api, job).unwrap();
    let attempts = api
        .store
        .list_backend_job_attempts(TEST_WORKSPACE_ID, &job.request.job_id)
        .unwrap();
    assert!(!attempts.is_empty());
    for attempt in attempts {
        assert_eq!(
            attempt.worker_cleanup_state,
            Some(BackendJobWorkerCleanupState::Completed)
        );
        api.require_backend_job_attempt_effects_ended(&attempt)
            .unwrap();
        if let Some(worker) = attempt.worker {
            assert!(api.runtime.worker(&worker).is_err());
            assert!(
                api.store
                    .get_worker_registry(TEST_WORKSPACE_ID, &worker)
                    .unwrap()
                    .is_none()
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn recovery_holds_stale_or_ambiguous_intents_without_blocking_independent_removal() {
    for ambiguous in [false, true] {
        let f = RemovalFixture::new().await;
        f.seed_active(vec![RemovalFixture::history(
            "independent",
            "saved before notification",
        )]);
        let subjects = open_subjektiv_store(f.api()).unwrap();
        let subject_a = create_journal_subject_as_owner(f.api(), "held-independent-body").await;
        let a = RemovalFixture::create_body(f.api(), &subject_a.id, false).await;
        RemovalFixture::stop_body(f.api(), &a).await;
        let plan = |worker: &RuntimeWorkerRef| {
            let inventory = f.api().runtime.worker_retention_inventory(worker).unwrap();
            f.api()
                .config_store
                .plan_worker_removal(
                    &crate::retention::WorkerRemovalPlanRequest {
                        workspace_id: TEST_WORKSPACE_ID.into(),
                        worker: worker.clone(),
                        reason: "saved removal intent before Journal initialization".into(),
                        journal_erasure: None,
                    },
                    &inventory,
                )
                .unwrap()
        };
        let first_a = plan(&a);
        let mut registry = f
            .api()
            .store
            .get_worker_registry(TEST_WORKSPACE_ID, &a)
            .unwrap()
            .unwrap();
        registry.updated_at = "changed-after-plan".into();
        f.api().store.upsert_worker_registry(&registry).unwrap();
        if ambiguous {
            assert_ne!(plan(&a).operation_id, first_a.operation_id);
        }
        let b = plan(&f.source);
        let a_rows = || {
            f.api().config_store.with_conn(|conn| {
                conn.prepare("SELECT operation_id,state,journal_removal_json FROM worker_removal_operations WHERE worker_id=?1 ORDER BY operation_id")?
                    .query_map([&a.worker_id], |row| Ok((row.get::<_,String>(0)?, row.get::<_,String>(1)?, row.get::<_,Option<String>>(2)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>().map_err(crate::Error::from)
            }).unwrap()
        };
        let held_before = a_rows();
        assert!(
            held_before
                .iter()
                .all(|(_, state, metadata)| state == "planned" && metadata.is_none())
        );
        f.model.set(ModelAction::Record);
        // Same startup/reconnect entry as production, not a second Remove request.
        subjektiv_journal::recover(f.api()).unwrap();
        f.wait_removed().await;
        f.wait_recorded().await;
        assert_eq!(f.prepared().plan.operation_id, b.operation_id);
        assert_eq!(
            a_rows(),
            held_before,
            "held intent must not execute or be rewritten"
        );
        assert!(f.api().runtime.worker(&a).is_ok());
        assert!(
            f.api()
                .store
                .get_worker_registry(TEST_WORKSPACE_ID, &a)
                .unwrap()
                .is_some()
        );
        assert!(
            !subjects
                .journal_worker_was_released(&subject_a.id, &a.runtime_id, &a.worker_id)
                .unwrap()
        );
        assert!(
            f.api()
                .config
                .embedded_runtime_store_root
                .join("workers")
                .join(&a.worker_id)
                .join("session")
                .exists()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn removed_body_history_is_not_reauthorized_on_reconnect_with_active_successor() {
    let f = RemovalFixture::new().await;
    f.seed_active(vec![RemovalFixture::history(
        "historical",
        "retained original",
    )]);
    assert_eq!(f.remove().await.status, StatusCode::OK.as_u16());
    f.wait_for_input().await;
    f.wait_removed().await;
    f.assert_removed_with_pending_recording();
    let successor = RemovalFixture::create_body(f.api(), &f.attr.subject_id, false).await;
    f.model.set(ModelAction::Record);
    // Historical-body recovery is a no-op only after retained recording and real
    // Job teardown finish; producer removal by itself no longer implies either.
    f.wait_recorded().await;
    assert_eq!(
        f.api()
            .store
            .require_current_worker_singleton_owner(TEST_WORKSPACE_ID, &successor)
            .unwrap()
            .unwrap()
            .worker,
        successor
    );
    assert!(
        f.api()
            .store
            .require_current_worker_singleton_owner(TEST_WORKSPACE_ID, &f.source)
            .is_err()
    );
    let store = open_subjektiv_store(f.api()).unwrap();
    let host =
        subjektiv::journal::JournalHostContext::validated_session(&store, f.attr.clone()).unwrap();
    let journal = store.journal_for_session(&host).unwrap().unwrap();
    let before_state = store.read_journal(&host, &journal).unwrap();
    let before_jobs = serde_json::to_value(
        f.api()
            .store
            .list_backend_journal_jobs(TEST_WORKSPACE_ID, &f.attr.session_id, None, 100)
            .unwrap(),
    )
    .unwrap();
    let before_inputs = f.model.attempt_inputs.lock().unwrap().len();
    #[derive(Clone)]
    struct Captured(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Captured {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let captured = Arc::new(Mutex::new(Vec::new()));
    let output = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(move || Captured(output.clone()))
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        for _ in 0..2 {
            subjektiv_journal::recover(f.api()).unwrap();
        }
    });
    let logs = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    assert!(
        !logs.contains("retained Journal prefix recovery blocked"),
        "historical body was treated as a live dispatch candidate"
    );
    assert_eq!(store.read_journal(&host, &journal).unwrap(), before_state);
    assert_eq!(
        serde_json::to_value(
            f.api()
                .store
                .list_backend_journal_jobs(TEST_WORKSPACE_ID, &f.attr.session_id, None, 100)
                .unwrap()
        )
        .unwrap(),
        before_jobs
    );
    assert_eq!(f.model.attempt_inputs.lock().unwrap().len(), before_inputs);
    assert!(
        store
            .journal_worker_was_released(
                &f.attr.subject_id,
                &f.source.runtime_id,
                &f.source.worker_id
            )
            .unwrap()
    );
    assert_eq!(
        store
            .session_attribution(&f.attr.session_id)
            .unwrap()
            .unwrap(),
        f.attr
    );
    assert!(f.api().runtime.worker(&f.source).is_err());
    assert!(f.api().runtime.worker(&successor).is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn stopped_removal_hands_off_originals_before_recording_and_recovers_without_producer() {
    let f = RemovalFixture::new().await;
    f.seed_active(vec![RemovalFixture::history(
        "unnotified",
        "saved without a recording notification",
    )]);
    let restore_token = f
        .api()
        .runtime
        .worker(&f.source)
        .unwrap()
        .restore_observation_token
        .unwrap();
    let response = f.remove().await;
    assert_eq!(
        response.status,
        StatusCode::OK.as_u16(),
        "{}",
        response.body
    );
    let body: Value = serde_json::from_str(&response.body).unwrap();
    assert_eq!(body["removed"], true);
    f.wait_for_input().await;
    f.wait_removed().await;
    assert!(
        !f.api()
            .store
            .list_retained_journal_recordings(TEST_WORKSPACE_ID, None, 100)
            .unwrap()
            .is_empty()
    );
    let prepared = f.prepared();
    let replay = f.remove().await;
    assert_eq!(replay.status, StatusCode::OK.as_u16(), "{}", replay.body);
    assert_eq!(f.prepared().plan.operation_id, prepared.plan.operation_id);
    let request = runtime_api::WorkerRestoreRequest {
        expected_observation_token: restore_token,
        request_id: "reject-restore-after-removal-freeze".into(),
        preparation: None,
    };
    let restored = f.api().runtime.restore_worker(&f.source, request);
    assert!(
        !matches!(&restored, Ok(result) if result.state == server_api::WorkerRestoreState::Accepted),
        "freeze rejects resurrection: {restored:?}"
    );
    let successor = RemovalFixture::create_body(f.api(), &f.attr.subject_id, false).await;
    assert!(f.api().runtime.worker(&successor).is_ok());
    f.model.set(ModelAction::Record);
    // Recording proceeds from retained originals after the producer is gone.
    f.wait_recorded().await;
    let store = open_subjektiv_store(f.api()).unwrap();
    assert!(
        store
            .journal_worker_was_released(
                &f.attr.subject_id,
                &f.source.runtime_id,
                &f.source.worker_id
            )
            .unwrap()
    );
    let host = JournalHostContext::validated_session(&store, f.attr.clone()).unwrap();
    assert!(!store.journal_retention_required(&host).unwrap());
    let jobs = f
        .api()
        .store
        .list_backend_journal_jobs(TEST_WORKSPACE_ID, &f.attr.session_id, None, 100)
        .unwrap();
    assert!(!jobs.is_empty());
    for job in &jobs {
        assert_eq!(job.state, BackendJobState::Completed);
        subjektiv_journal::require_job_effects_ended(f.api(), job).unwrap();
        assert!(
            f.api()
                .store
                .list_backend_job_attempts(TEST_WORKSPACE_ID, &job.request.job_id)
                .unwrap()
                .iter()
                .all(|attempt| attempt.worker_cleanup_state
                    == Some(BackendJobWorkerCleanupState::Completed))
        );
    }
    let archive = f
        .api()
        .config_store
        .worker_session_archive(TEST_WORKSPACE_ID, &f.source, &f.attr.session_id)
        .unwrap()
        .unwrap();
    assert_eq!(archive.operation_id, prepared.plan.operation_id);
    let completed = f.prepared();
    assert_eq!(completed.plan.operation_id, prepared.plan.operation_id);
    assert_eq!(
        completed.plan.state,
        crate::retention::WorkerRemovalPlanState::Succeeded
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn unnotified_multisession_multisegment_utf8_history_chains_bounded_jobs_without_gaps() {
    let f = RemovalFixture::new().await;
    let long = "界\\\"\n".repeat(28_000);
    f.seed_active(vec![RemovalFixture::history("active", &long)]);
    let retained = f.seed_retained(vec![
        RemovalFixture::history("repeated-original", &long),
        RemovalFixture::history(
            &format!("metadata-{}", "m".repeat(100)),
            "earlier Segment original outside the selected active prefix",
        ),
    ]);
    let newest = open_subjektiv_store(f.api())
        .unwrap()
        .subject_session_attribution_page(
            &f.attr.subject_id,
            None,
            &Utc::now().to_rfc3339(),
            None,
            100,
        )
        .unwrap();
    assert_eq!(
        newest.items.first().unwrap().session_id,
        retained.session_id,
        "the latest attribution is intentionally not the physical primary Session"
    );
    let response = f.remove().await;
    assert_eq!(
        response.status,
        StatusCode::OK.as_u16(),
        "{}",
        response.body
    );
    f.wait_for_input().await;
    f.wait_removed().await;
    let prepared = f.prepared();
    let progress = f
        .api()
        .config_store
        .get_journal_removal(&prepared)
        .unwrap()
        .unwrap();
    assert_eq!(progress.metadata.members.as_ref().unwrap().len(), 2);
    assert_eq!(
        progress
            .metadata
            .members
            .as_ref()
            .unwrap()
            .iter()
            .find(|member| member.session_id == retained.session_id)
            .unwrap()
            .end
            .canonical_order,
        3,
        "the active prefix plus detached originals excludes repeated stable history"
    );
    let (first_job_id, first_input) = f.model.inputs.lock().unwrap()[0].clone();
    let selected = first_input.selected_ranges.last().unwrap();
    assert!(
        selected.bytes.end < selected.source.total_bytes,
        "the first completed Job must still leave a frozen member suffix"
    );
    f.model.set(ModelAction::RecordFirstPrefix);
    // Whole-Job completion now includes actual Controller teardown and resource
    // release, rather than ending at the result callback.
    let first_done = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let job = f
                .api()
                .store
                .get_backend_job(TEST_WORKSPACE_ID, &first_job_id)
                .unwrap()
                .unwrap();
            if matches!(
                job.state,
                BackendJobState::Completed | BackendJobState::Failed
            ) {
                break job;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        let job = f
            .api()
            .store
            .get_backend_job(TEST_WORKSPACE_ID, &first_job_id)
            .unwrap()
            .unwrap();
        let attempts = f
            .api()
            .store
            .list_backend_job_attempts(TEST_WORKSPACE_ID, &first_job_id)
            .unwrap();
        panic!(
            "first bounded prefix stalled: state={:?}, failure={:?}; attempts={:?}",
            job.state,
            job.failure_category,
            attempts
                .iter()
                .map(|a| (
                    &a.state,
                    &a.failure_category,
                    &a.worker_cleanup_state,
                    &a.worker_cleanup_failure_category,
                    &a.worker_cleanup_failure_detail,
                    a.worker
                        .as_ref()
                        .and_then(|w| f.api().runtime.worker(w).ok())
                        .map(|w| (w.state, w.worker_state)),
                    &a.runtime_run_id,
                    f.api()
                        .store
                        .has_backend_job_execution_fence(TEST_WORKSPACE_ID, a)
                        .unwrap(),
                    a.worker
                        .as_ref()
                        .and_then(|w| f
                            .api()
                            .config_store
                            .recover_worker_removal_execution(TEST_WORKSPACE_ID, w)
                            .ok()
                            .flatten())
                        .map(|p| (p.plan.state, p.prior_failure_category))
                ))
                .collect::<Vec<_>>()
        );
    });
    assert_eq!(
        first_done.state,
        BackendJobState::Completed,
        "first prefix failure: {:?}",
        (&first_done.failure_category, &first_done.failure_detail)
    );
    f.wait_removed().await;
    let prefix_progress = f
        .api()
        .config_store
        .get_journal_removal(&prepared)
        .unwrap()
        .unwrap();
    assert_eq!(
        prefix_progress.metadata.phase,
        crate::store::JournalRemovalPhase::Completed
    );
    assert!(
        !prefix_progress
            .metadata
            .retained_recording
            .as_ref()
            .unwrap()
            .completed,
        "one bounded prefix must not complete the retained all-member recording responsibility"
    );
    f.model.set(ModelAction::Record);
    f.wait_recorded().await;
    let receipt = f
        .api()
        .runtime
        .execute_worker_retention(&f.source, prepared.runtime_request.clone())
        .unwrap();
    let archive = receipt.archive.as_ref().unwrap();
    assert_eq!(archive.operation_id, prepared.plan.operation_id);
    assert_eq!(
        archive.source_session_id, f.attr.session_id,
        "physical primary identity must not follow newest attribution ordering"
    );
    let expected_members = progress
        .metadata
        .members
        .as_ref()
        .unwrap()
        .iter()
        .map(|member| (member.session_id.clone(), member.active_segment_id))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut archived_members = std::collections::BTreeMap::from([(
        archive.source_session_id.clone(),
        archive.source_active_segment_id,
    )]);
    for member in &archive.retained_sessions {
        assert_eq!(member.owner.workspace_id, TEST_WORKSPACE_ID);
        assert_eq!(member.owner.worker_id, f.source.worker_id);
        assert!(
            archived_members
                .insert(
                    member.session_id.to_string(),
                    Some(member.active_segment_id)
                )
                .is_none()
        );
    }
    assert_eq!(
        archived_members, expected_members,
        "one immutable archive must preserve every frozen member"
    );
    for member in progress.metadata.members.as_ref().unwrap() {
        let catalog = f
            .api()
            .runtime
            .session_material_catalog(
                &f.source.runtime_id,
                runtime_api::SessionMaterialCatalogRequest {
                    workspace_id: TEST_WORKSPACE_ID.into(),
                    source: runtime_api::SessionPublicSource::Archived {
                        archive_id: archive.archive_id.clone(),
                    },
                    expected_session_id: member.session_id.clone(),
                    expected_catalog_sha256: None,
                    offset: 0,
                    limit: 100,
                },
            )
            .unwrap();
        assert!(
            matches!(&catalog, runtime_api::SessionMaterialCatalogAvailability::Page { page } if !page.descriptors.is_empty()),
            "each archived primary/retained member stays scoped-readable: {catalog:?}"
        );
        for descriptor in &member.descriptors {
            assert!(
                archive
                    .segment_ids
                    .contains(&descriptor.identity.segment_id.to_string()),
                "archive must also preserve detached original Segments, not only active prefixes"
            );
        }
    }
    let inputs = f.model.inputs.lock().unwrap();
    assert!(
        inputs.len() > 2,
        "history exceeds a real immutable Job input"
    );
    assert!(
        inputs.iter().any(|(_, input)| input
            .capture
            .entries
            .iter()
            .map(|entry| match &entry.body {
                session_store::capture::CaptureBody::CanonicalPublic { text, .. } => text.len(),
                _ => 0,
            })
            .sum::<usize>()
            > 48 * 1024),
        "48KiB must not remain the capture ceiling"
    );
    for (_, input) in inputs.iter() {
        assert!(
            serde_json::to_vec(input).unwrap().len() <= crate::backend_job::MAX_JOB_INPUT_BYTES,
            "the immutable envelope, including metadata and escaped UTF-8, must fit the real Job contract"
        );
    }
    for attr in [&f.attr, &retained] {
        let store = open_subjektiv_store(f.api()).unwrap();
        let host = JournalHostContext::validated_session(&store, attr.clone()).unwrap();
        let id = store.journal_for_session(&host).unwrap().unwrap();
        let journal = store.read_journal(&host, &id).unwrap();
        let member = progress
            .metadata
            .members
            .as_ref()
            .unwrap()
            .iter()
            .find(|member| member.session_id == attr.session_id)
            .unwrap();
        assert_eq!(
            (journal.process_order, journal.process_byte),
            (member.end.canonical_order, member.end.byte_offset)
        );
        assert!(!store.journal_retention_required(&host).unwrap());
        let mut selected = inputs
            .iter()
            .filter(|(_, input)| input.capture.session_id == attr.session_id)
            .flat_map(|(_, input)| input.selected_ranges.clone())
            .collect::<Vec<_>>();
        selected.sort_by_key(|range| (range.source.order, range.bytes.start));
        let mut next = (0, 0);
        for range in &selected {
            assert_eq!(
                (range.source.order, range.bytes.start),
                next,
                "no skipped or duplicate original bytes"
            );
            next = if range.bytes.end == range.source.total_bytes {
                (range.source.order + 1, 0)
            } else {
                (range.source.order, range.bytes.end)
            };
        }
        assert_eq!(next, (member.end.canonical_order, member.end.byte_offset));
        let mut pieces = inputs
            .iter()
            .filter(|(_, input)| input.capture.session_id == attr.session_id)
            .flat_map(|(_, input)| {
                input
                    .selected_ranges
                    .iter()
                    .cloned()
                    .zip(input.capture.entries.iter().cloned())
            })
            .collect::<Vec<_>>();
        pieces.sort_by_key(|(range, _)| (range.source.order, range.bytes.start));
        let mut originals = std::collections::BTreeMap::<u64, (JournalSource, String)>::new();
        for (range, entry) in pieces {
            let session_store::capture::CaptureBody::CanonicalPublic { text, .. } = entry.body
            else {
                panic!("recording must receive immutable public bodies")
            };
            let original = originals
                .entry(range.source.order)
                .or_insert_with(|| (range.source.clone(), String::new()));
            assert_eq!(original.1.len() as u64, range.bytes.start);
            original.1.push_str(&text);
        }
        for (_, (source, text)) in originals {
            assert_eq!(text.len() as u64, source.total_bytes);
            assert_eq!(
                Sha256::digest(text.as_bytes())
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>(),
                source.content_sha256,
                "reassembled real model input must match the frozen original identity"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn unavailable_original_survives_restart_and_same_removal_resumes_without_new_request() {
    let mut f = RemovalFixture::new().await;
    f.seed_active(vec![RemovalFixture::history(
        "unavailable",
        "must remain pending while source cannot be read",
    )]);
    let hidden = f.root().with_file_name("temporarily-unavailable-source");
    std::fs::rename(f.root(), &hidden).unwrap();
    let response = f.remove().await;
    assert_eq!(
        response.status,
        StatusCode::ACCEPTED.as_u16(),
        "{}",
        response.body
    );
    let unavailable: Value = serde_json::from_str(&response.body).unwrap();
    assert_eq!(unavailable["removed"], false);
    assert_eq!(unavailable["state"], "held");
    assert_eq!(unavailable["cause"], "source_unavailable");
    let original = f.prepared();
    assert!(f.model.inputs.lock().unwrap().is_empty());
    assert!(
        !open_subjektiv_store(f.api())
            .unwrap()
            .journal_worker_was_released(
                &f.attr.subject_id,
                &f.source.runtime_id,
                &f.source.worker_id
            )
            .unwrap()
    );
    std::fs::rename(hidden, f.root()).unwrap();
    f.model.set(ModelAction::Record);
    f.restart().await;
    f.wait_removed().await;
    f.wait_recorded().await;
    assert_eq!(f.prepared().plan.operation_id, original.plan.operation_id);
    assert_eq!(
        f.prepared().plan.input_fingerprint,
        original.plan.input_fingerprint
    );
}

async fn browser_recording(f: &RemovalFixture) -> Value {
    let token = seed_test_owner_api_token(
        f.api(),
        &format!("recording-observation-{}", Uuid::new_v4()),
    );
    let request = axum::http::Request::builder()
        .method("POST")
        .uri(format!(
            "/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{}/journal-browser",
            f.attr.subject_id
        ))
        .header(CONTENT_TYPE, "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(
            r#"{"action":"sessions","after":null,"limit":32}"#,
        ))
        .unwrap();
    let response = tower::ServiceExt::oneshot(build_router(f.api().clone()), request)
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), MAX_JOURNAL_RESPONSE_BYTES)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    println!(
        "JOURNAL_BROWSER_CONTRACT {}",
        json!({
            "workspace_id": TEST_WORKSPACE_ID,
            "subject_id": f.attr.subject_id,
            "request": {"action":"sessions", "after":null, "limit":32},
            "response": body,
        })
    );
    body["value"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["journal"]["source_session_id"] == f.attr.session_id)
        .expect("recorded Session remains browseable")["retained_recording"]
        .clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn failed_recording_releases_producer_but_retains_input_checkpoint_and_failure() {
    let f = RemovalFixture::new().await;
    f.seed_active(vec![RemovalFixture::history(
        "failed",
        "a failed Job is not recording coverage",
    )]);
    // Set a short ORIGINAL deadline at reservation, before any owner observes it.
    // Never mutate a running owner's deadline/CAS snapshot.
    let conn = rusqlite::Connection::open(&f.api().config.database_path).unwrap();
    conn.execute_batch("CREATE TRIGGER short_journal_deadline AFTER INSERT ON backend_job_attempts WHEN NEW.job_id LIKE 'journal-recording:%' BEGIN UPDATE backend_job_attempts SET deadline_at=strftime('%Y-%m-%dT%H:%M:%fZ','now','+15 seconds') WHERE workspace_id=NEW.workspace_id AND attempt_id=NEW.attempt_id; END;").unwrap();
    drop(conn);
    let response = f.remove().await;
    assert_eq!(
        response.status,
        StatusCode::OK.as_u16(),
        "{}",
        response.body
    );
    f.wait_for_input().await;
    f.wait_removed().await;
    f.assert_removed_with_pending_recording();
    let original_job = f
        .api()
        .store
        .list_backend_journal_jobs(TEST_WORKSPACE_ID, &f.attr.session_id, None, 100)
        .unwrap()
        .pop()
        .unwrap();
    let _attempt = f
        .api()
        .store
        .get_backend_job_attempt(
            TEST_WORKSPACE_ID,
            &original_job.request.job_id,
            &crate::backend_job::attempt_id(
                &original_job.request.job_id,
                original_job.current_attempt,
            ),
        )
        .unwrap()
        .unwrap();
    f.model.set(ModelAction::Fail);
    tokio::time::timeout(Duration::from_secs(10), f.model.rejected.notified())
        .await
        .expect("scripted model boundary returned rejection, not completion or termination");
    f.wait_removed().await;
    let store = open_subjektiv_store(f.api()).unwrap();
    let host = JournalHostContext::validated_session(&store, f.attr.clone()).unwrap();
    let journal_id = store.journal_for_session(&host).unwrap().unwrap();
    let unchanged = store.read_journal(&host, &journal_id).unwrap();
    assert_eq!((unchanged.process_order, unchanged.process_byte), (0, 0));
    assert_ne!(
        f.api()
            .store
            .get_backend_job(TEST_WORKSPACE_ID, &original_job.request.job_id)
            .unwrap()
            .unwrap()
            .state,
        BackendJobState::Completed
    );
    // The real original deadline owner must perform timeout, Stop, drain and cleanup.
    // The rejection callback is not Controller or Host termination evidence.
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let jobs = f
                .api()
                .store
                .list_backend_journal_jobs(TEST_WORKSPACE_ID, &f.attr.session_id, None, 100)
                .unwrap();
            if jobs.iter().any(|job| job.state == BackendJobState::Failed) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the real Job deadline owner records timeout as Failed");
    f.wait_removed().await;
    let before = f.prepared();
    let retry = f.remove().await;
    assert_eq!(retry.status, StatusCode::OK.as_u16(), "{}", retry.body);
    let held: Value = serde_json::from_str(&retry.body).unwrap();
    assert_eq!(held["removed"], true);
    subjektiv_journal::resume_removals(f.api()).unwrap();
    let responsibility = f
        .api()
        .store
        .list_retained_journal_recordings(TEST_WORKSPACE_ID, None, 100)
        .unwrap();
    assert_eq!(responsibility.len(), 1);
    let recording = responsibility[0]
        .metadata
        .retained_recording
        .as_ref()
        .unwrap();
    assert!(!recording.completed);
    assert_eq!(
        recording.cause,
        Some(crate::store::JournalRemovalCause::JobFailed)
    );
    assert_eq!(
        browser_recording(&f).await,
        json!({
            "producer": "removed", "completed": false, "cause": "job_failed"
        })
    );
    assert_eq!(f.prepared().plan.operation_id, before.plan.operation_id);
    f.wait_removed().await;
    let jobs = f
        .api()
        .store
        .list_backend_journal_jobs(TEST_WORKSPACE_ID, &f.attr.session_id, None, 100)
        .unwrap();
    assert!(
        jobs.iter()
            .all(|job| job.state != BackendJobState::Completed)
    );
    assert!(jobs.iter().any(|job| job.load_executable_request().is_ok()));
    assert_eq!(jobs.len(), 1, "failure must not select a replacement Job");
    assert_eq!(jobs[0].state, BackendJobState::Failed);
    assert_eq!(
        jobs[0].failure_category.as_deref(),
        Some("timeout"),
        "retained failure: {:?}",
        jobs[0].failure_detail
    );
    assert_job_teardown(f.api(), &jobs[0]);
    f.assert_removed_with_pending_recording();
    assert!(matches!(
        store.active_journal_generation(&host, &journal_id),
        Err(subjektiv::journal::JournalError::PermissionDenied(_))
    ));
    assert_no_active_journal_generation(f.api(), &journal_id);
    assert_eq!(jobs[0].input_digest, original_job.input_digest);
    assert!(
        jobs[0].load_executable_request().unwrap().input
            == original_job.load_executable_request().unwrap().input,
        "failure preserves the complete saved input (body omitted)"
    );
    let after_timeout = store.read_journal(&host, &journal_id).unwrap();
    assert_eq!(
        (after_timeout.process_order, after_timeout.process_byte),
        (unchanged.process_order, unchanged.process_byte)
    );
    assert!(store.journal_retention_required(&host).unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn completed_checkpoint_with_failed_job_cleanup_keeps_job_finalizing_not_source_occupied() {
    let f = RemovalFixture::new().await;
    f.seed_active(vec![RemovalFixture::history(
        "cleanup-delayed",
        "checkpoint is not cleanup confirmation",
    )]);
    let response = f.remove().await;
    assert_eq!(
        response.status,
        StatusCode::OK.as_u16(),
        "{}",
        response.body
    );
    f.wait_for_input().await;
    f.wait_removed().await;
    f.assert_removed_with_pending_recording();
    let jobs = f
        .api()
        .store
        .list_backend_journal_jobs(TEST_WORKSPACE_ID, &f.attr.session_id, None, 100)
        .unwrap();
    assert_eq!(jobs.len(), 1);
    let job = &jobs[0];
    let attempt = f
        .api()
        .store
        .get_backend_job_attempt(
            TEST_WORKSPACE_ID,
            &job.request.job_id,
            &crate::backend_job::attempt_id(&job.request.job_id, job.current_attempt),
        )
        .unwrap()
        .unwrap();
    let worker = attempt.worker.as_ref().unwrap();
    // Fail the real catalog commit after Runtime Purge. Retry must recover that
    // exact child receipt, not replace a policy-blocked plan with a new operation.
    let conn = rusqlite::Connection::open(&f.api().config.database_path).unwrap();
    conn.execute_batch(&format!(
        "CREATE TRIGGER interrupt_job_cleanup BEFORE DELETE ON worker_registry WHEN OLD.worker_id='{}' BEGIN SELECT RAISE(ABORT,'injected cleanup catalog failure'); END;",
        worker.worker_id
    )).unwrap();
    f.model.set(ModelAction::Record);
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let current = f
                .api()
                .store
                .get_backend_job_attempt(
                    TEST_WORKSPACE_ID,
                    &job.request.job_id,
                    &attempt.attempt_id,
                )
                .unwrap()
                .unwrap();
            if current.worker_cleanup_state == Some(BackendJobWorkerCleanupState::Failed) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("actual cleanup reaches the injected catalog failure");
    let completed = f
        .api()
        .store
        .get_backend_job(TEST_WORKSPACE_ID, &job.request.job_id)
        .unwrap()
        .unwrap();
    assert_eq!(completed.state, BackendJobState::Finalizing);
    assert!(
        completed.result.is_some(),
        "accepted result survives failed teardown"
    );
    assert!(subjektiv_journal::require_job_effects_ended(f.api(), &completed).is_err());
    let cleanup_plan = f
        .api()
        .config_store
        .recover_worker_removal_execution(TEST_WORKSPACE_ID, worker)
        .unwrap()
        .unwrap();
    assert_eq!(
        cleanup_plan.plan.state,
        crate::retention::WorkerRemovalPlanState::Failed
    );
    assert_eq!(
        cleanup_plan.prior_failure_category.as_deref(),
        Some("metadata_commit_failed")
    );
    assert!(
        f.api().runtime.worker(worker).is_err(),
        "real Runtime teardown precedes the failed catalog commit"
    );
    assert!(
        f.api()
            .store
            .get_worker_registry(TEST_WORKSPACE_ID, worker)
            .unwrap()
            .is_some()
    );
    let store = open_subjektiv_store(f.api()).unwrap();
    let host = JournalHostContext::validated_session(&store, f.attr.clone()).unwrap();
    let journal_id = store.journal_for_session(&host).unwrap().unwrap();
    let journal = store.read_journal(&host, &journal_id).unwrap();
    let progress = f
        .api()
        .config_store
        .get_journal_removal(&f.prepared())
        .unwrap()
        .unwrap();
    let end = progress.metadata.members.as_ref().unwrap()[0].end;
    assert_eq!(
        (journal.process_order, journal.process_byte),
        (end.canonical_order, end.byte_offset),
        "all frozen bytes have real checkpoints, independently of generation/Job cleanup"
    );
    f.assert_removed_with_pending_recording();
    let prepared = f.prepared();
    conn.execute_batch("DROP TRIGGER interrupt_job_cleanup")
        .unwrap();
    drop(conn);
    let failed_cleanup = f
        .api()
        .store
        .get_backend_job_attempt(TEST_WORKSPACE_ID, &job.request.job_id, &attempt.attempt_id)
        .unwrap()
        .unwrap();
    // Failed is persisted before the old cleanup owner releases its lease.
    // Wait for that owner to return; a duplicate scheduling call is not a retry.
    tokio::time::timeout(Duration::from_secs(10), async {
        while f
            .api()
            .backend_job_recovery
            .cleanup_owner_ids()
            .contains(&attempt.attempt_id)
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("failed cleanup owner returns before explicit retry");
    assert!(f.api().schedule_backend_job_worker_cleanup(&failed_cleanup));
    f.wait_recorded().await;
    let finalized = f
        .api()
        .store
        .get_backend_job(TEST_WORKSPACE_ID, &job.request.job_id)
        .unwrap()
        .unwrap();
    assert_eq!(finalized.state, BackendJobState::Completed);
    assert_eq!(finalized.result, completed.result);
    assert_eq!(finalized.input_digest, completed.input_digest);
    assert_eq!(finalized.current_attempt, completed.current_attempt);
    assert_job_teardown(f.api(), &finalized);
    assert!(!store.journal_retention_required(&host).unwrap());
    assert!(matches!(
        store.active_journal_generation(&host, &journal_id),
        Err(subjektiv::journal::JournalError::PermissionDenied(_))
    ));
    assert_no_active_journal_generation(f.api(), &journal_id);
    let recovered_cleanup = f
        .api()
        .config_store
        .recover_worker_removal_execution(TEST_WORKSPACE_ID, worker)
        .unwrap()
        .unwrap();
    assert_eq!(
        recovered_cleanup.plan.operation_id,
        cleanup_plan.plan.operation_id
    );
    assert_eq!(
        recovered_cleanup.plan.input_fingerprint,
        cleanup_plan.plan.input_fingerprint
    );
    assert_eq!(
        recovered_cleanup.plan.state,
        crate::retention::WorkerRemovalPlanState::Succeeded
    );
    f.wait_removed().await;
    assert_eq!(f.prepared().plan.operation_id, prepared.plan.operation_id);
    assert_eq!(
        f.prepared().plan.input_fingerprint,
        prepared.plan.input_fingerprint
    );
}

#[tokio::test]
async fn erased_unrecorded_job_allows_archived_source_removal_without_reconstruction_or_replacement()
 {
    let f = SubjectRemovalFixture::new().await;
    let reserved = f.pending_journal();
    f.api
        .store
        .reclaim_backend_job_input(
            TEST_WORKSPACE_ID,
            &reserved.job.request.job_id,
            &reserved.job.input_digest,
            crate::backend_job::JobInputRetirementReason::ExplicitErasure,
        )
        .unwrap();
    let before = f
        .api
        .store
        .get_backend_job(TEST_WORKSPACE_ID, &reserved.job.request.job_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        before.input_disposition,
        crate::backend_job::JobInputDisposition::Erased
    );
    f.execution.take_inputs();
    let response = f.remove();
    assert_eq!(
        response.status,
        StatusCode::OK.as_u16(),
        "{}",
        response.body
    );
    assert_eq!(
        serde_json::from_str::<Value>(&response.body).unwrap()["removed"],
        true
    );
    f.assert_removed_with_history();
    assert_retained_recording(&f.api, &f.target, &f.attr);
    let after = f
        .api
        .store
        .get_backend_job(TEST_WORKSPACE_ID, &reserved.job.request.job_id)
        .unwrap()
        .unwrap();
    assert_eq!(after.state, before.state);
    assert_eq!(after.current_attempt, before.current_attempt);
    assert_eq!(after.input_digest, before.input_digest);
    assert_eq!(after.failure_category, before.failure_category);
    assert!(after.result.is_none());
    assert!(before.load_executable_request().is_err());
    assert!(after.load_executable_request().is_err());
    assert!(
        f.execution.take_inputs().is_empty(),
        "erased input must not be reconstructed or dispatched"
    );
    assert_eq!(
        f.api
            .store
            .list_backend_journal_jobs(TEST_WORKSPACE_ID, &f.attr.session_id, None, 100)
            .unwrap()
            .len(),
        1
    );
    let journal_id = before
        .request
        .grants
        .subjektiv_journal
        .as_ref()
        .unwrap()
        .journal_id
        .clone();
    let journal = f.store.read_journal(&f.host, &journal_id).unwrap();
    assert_eq!((journal.process_order, journal.process_byte), (0, 0));
    assert!(f.store.journal_retention_required(&f.host).unwrap());
    assert!(
        !f.store.journal_session_erased(&f.host).unwrap(),
        "retiring one Job's input is not explicit erasure of its source Session"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn physical_archive_publication_failure_holds_source_then_restarts_original_handoff() {
    let mut f = RemovalFixture::new().await;
    f.seed_active(vec![RemovalFixture::history(
        "archive-retry",
        "record once; resume physical retention only",
    )]);
    let archive_parent = f
        .api()
        .config
        .embedded_runtime_store_root
        .join("archives/workers");
    std::fs::create_dir_all(archive_parent.parent().unwrap()).unwrap();
    std::fs::write(&archive_parent, "injected archive publication failure").unwrap();
    let response = f.remove().await;
    assert_eq!(
        response.status,
        StatusCode::ACCEPTED.as_u16(),
        "{}",
        response.body
    );
    // This filesystem fault is at publication, BEFORE handoff/Job dispatch. Do
    // not wait for a recording Controller that cannot legally have been started.
    let body: Value = serde_json::from_str(&response.body).unwrap();
    assert_eq!(body["removed"], false);
    assert_eq!(body["state"], "held");
    assert_eq!(body["cause"], "source_unavailable");
    let original = f.prepared();
    let progress = f
        .api()
        .config_store
        .get_journal_removal(&original)
        .unwrap()
        .unwrap();
    assert!(progress.metadata.retained_recording.is_none());
    assert!(
        !progress.metadata.members.as_ref().unwrap()[0]
            .descriptors
            .is_empty()
    );
    assert_ne!(
        progress.metadata.phase,
        crate::store::JournalRemovalPhase::Released
    );
    f.assert_held();
    assert!(f.model.inputs.lock().unwrap().is_empty());
    assert!(
        f.api()
            .store
            .list_backend_journal_jobs(TEST_WORKSPACE_ID, &f.attr.session_id, None, 100)
            .unwrap()
            .is_empty()
    );
    std::fs::remove_file(archive_parent).unwrap();
    f.restart().await;
    f.wait_removed().await;
    f.wait_for_input().await;
    f.assert_removed_with_pending_recording();
    let (job_id, input) = f.model.inputs.lock().unwrap()[0].clone();
    assert!(
        input
            .capture
            .entries
            .iter()
            .any(|entry| matches!(&entry.body,
        session_store::capture::CaptureBody::CanonicalPublic { text, .. }
            if text.contains("record once; resume physical retention only")))
    );
    f.model.set(ModelAction::Record);
    f.wait_recorded().await;
    let completed = f.prepared();
    assert_eq!(completed.plan.operation_id, original.plan.operation_id);
    assert_eq!(
        completed.plan.input_fingerprint,
        original.plan.input_fingerprint
    );
    assert_eq!(completed.plan.archive_id, original.plan.archive_id);
    assert_eq!(
        f.model
            .inputs
            .lock()
            .unwrap()
            .iter()
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>(),
        vec![job_id.clone()],
        "publication recovery records the saved original once, without restoring its producer"
    );
    let job = f
        .api()
        .store
        .get_backend_job(TEST_WORKSPACE_ID, &job_id)
        .unwrap()
        .unwrap();
    assert_eq!(job.state, BackendJobState::Completed);
    assert_job_teardown(f.api(), &job);
    let archive = f
        .api()
        .config_store
        .worker_session_archive(TEST_WORKSPACE_ID, &f.source, &f.attr.session_id)
        .unwrap()
        .unwrap();
    assert_eq!(archive.operation_id, original.plan.operation_id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn archived_removal_rejects_wrong_receipts_and_restarts_original_catalog_commit() {
    let mut f = RemovalFixture::new().await;
    f.seed_active(vec![RemovalFixture::history(
        "catalog-cut",
        "real checkpoint before removal and failed catalog commit",
    )]);
    f.seed_retained(vec![RemovalFixture::history(
        "catalog-retained",
        "retained Session must survive the same receipt",
    )]);
    // Finish the genuine archive/responsibility handoff before injecting failure
    // into the later source catalog transaction. No receipt or release is forged.
    let original = f.prepare_handoff();
    let conn = rusqlite::Connection::open(&f.api().config.database_path).unwrap();
    // Block only the original catalog commit, not its Journal Workers' real cleanup.
    conn.execute_batch(&format!(
        "CREATE TRIGGER interrupt_source_catalog BEFORE DELETE ON worker_registry
        WHEN OLD.workspace_id='{TEST_WORKSPACE_ID}' AND OLD.worker_id='{}'
        BEGIN SELECT RAISE(ABORT,'injected original catalog commit failure'); END;",
        f.source.worker_id
    ))
    .unwrap();
    let response = f.remove().await;
    assert_eq!(
        response.status,
        StatusCode::SERVICE_UNAVAILABLE.as_u16(),
        "{}",
        response.body
    );
    assert_eq!(
        serde_json::from_str::<Value>(&response.body).unwrap()["code"],
        "worker_removal_authority_unavailable"
    );
    f.wait_for_input().await;
    let failed = f.prepared();
    assert_eq!(
        failed.plan.state,
        crate::retention::WorkerRemovalPlanState::Failed
    );
    assert_eq!(
        failed.prior_failure_category.as_deref(),
        Some("metadata_commit_failed")
    );
    assert!(
        !f.root().exists(),
        "physical retention finished before the failed catalog transaction"
    );
    assert!(f.api().runtime.worker(&f.source).is_err());
    let progress = f
        .api()
        .config_store
        .get_journal_removal(&failed)
        .unwrap()
        .unwrap();
    assert_eq!(
        progress.metadata.phase,
        crate::store::JournalRemovalPhase::Released
    );
    assert!(
        !progress
            .metadata
            .retained_recording
            .as_ref()
            .unwrap()
            .completed
    );
    f.model.set(ModelAction::Record);
    f.wait_recorded().await;
    let store = open_subjektiv_store(f.api()).unwrap();
    assert!(
        store
            .journal_worker_was_released(
                &f.attr.subject_id,
                &f.source.runtime_id,
                &f.source.worker_id
            )
            .unwrap()
    );
    let key = crate::subjektiv::subject_worker_singleton_key(&f.attr.subject_id).unwrap();
    assert_eq!(
        f.api()
            .store
            .current_worker_singleton_owner(TEST_WORKSPACE_ID, &key)
            .unwrap()
            .unwrap()
            .worker,
        f.source,
        "a Runtime retention receipt is not the Backend catalog commit"
    );
    let prepared = f.prepared();
    let result = f
        .api()
        .runtime
        .execute_worker_retention(&f.source, prepared.runtime_request.clone())
        .unwrap();
    let service = WorkerRemovalService::new(f.api());
    for variant in 0..8 {
        let mut wrong = result.clone();
        match variant {
            0 => wrong.source_removed = false,
            1 => wrong.operation_id.push_str("-wrong"),
            2 => wrong.input_fingerprint.push_str("-wrong"),
            3 => wrong
                .archive
                .as_mut()
                .unwrap()
                .source_session_id
                .push_str("-wrong"),
            4 => wrong.archive.as_mut().unwrap().retained_sessions.clear(),
            5 => {
                wrong.archive.as_mut().unwrap().retained_sessions[0]
                    .owner
                    .worker_id = Uuid::new_v4().to_string()
            }
            6 => {
                let archive = wrong.archive.as_mut().unwrap();
                archive.retained_sessions[0].session_id =
                    archive.source_session_id.parse().unwrap();
            }
            _ => wrong.archive = None,
        }
        let mut committed = false;
        assert!(
            subjektiv_journal::finish_worker_removal(
                &service,
                &f.source,
                &prepared,
                &wrong,
                || committed = true
            )
            .is_err()
        );
        assert!(
            !committed,
            "wrong receipt variant {variant} must not reach catalog commit"
        );
        assert_eq!(
            f.api()
                .store
                .current_worker_singleton_owner(TEST_WORKSPACE_ID, &key)
                .unwrap()
                .unwrap()
                .worker,
            f.source,
            "wrong receipt must not release source occupancy"
        );
        assert!(
            f.api()
                .config_store
                .worker_session_archive(TEST_WORKSPACE_ID, &f.source, &f.attr.session_id)
                .unwrap()
                .is_none(),
            "wrong receipt must not publish a Backend archive record"
        );
        assert!(
            f.api()
                .store
                .get_worker_registry(TEST_WORKSPACE_ID, &f.source)
                .unwrap()
                .is_some()
        );
        assert_eq!(f.prepared().plan.operation_id, original.plan.operation_id);
    }
    // Positive control: the same real receipt reaches the commit boundary. The
    // corrupt variants above must fail on their own identity/scope mismatch.
    let mut matching_receipt = false;
    subjektiv_journal::finish_worker_removal(&service, &f.source, &prepared, &result, || {
        matching_receipt = true
    })
    .unwrap();
    assert!(matching_receipt);
    conn.execute_batch("DROP TRIGGER interrupt_source_catalog;")
        .unwrap();
    drop(conn);
    drop(store);
    drop(service);
    let model_jobs = f.model.inputs.lock().unwrap().len();
    f.restart().await;
    f.wait_removed().await;
    let completed = f.prepared();
    assert_eq!(completed.plan.operation_id, original.plan.operation_id);
    assert_eq!(
        completed.plan.input_fingerprint,
        original.plan.input_fingerprint
    );
    assert_eq!(completed.plan.archive_id, original.plan.archive_id);
    let saved = f
        .api()
        .config_store
        .worker_session_archive(TEST_WORKSPACE_ID, &f.source, &f.attr.session_id)
        .unwrap()
        .unwrap();
    assert_eq!(saved.operation_id, result.operation_id);
    assert_eq!(saved.archive_id, result.archive.unwrap().archive_id);
    assert_eq!(
        f.model.inputs.lock().unwrap().len(),
        model_jobs,
        "resume must not record originals twice"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn pending_original_model_and_accepted_queue_are_not_stopped_or_discarded_by_remove() {
    let f = RemovalFixture::with_live_source(true).await;
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let changed = f.model.source_changed.notified();
            if f.model
                .source_requests
                .lock()
                .unwrap()
                .iter()
                .any(|request| {
                    request
                        .items
                        .iter()
                        .filter_map(Item::as_text)
                        .any(|text| text.contains("pending-original-model-turn"))
                })
            {
                break;
            }
            changed.await;
        }
    })
    .await
    .expect("the actual original Controller is held at its external model request");
    let queued_id = "accepted-queued-original-before-remove";
    let queued_text = "accepted-queued-original-must-reach-model-and-checkpoint";
    let runtime = f.api().runtime.clone();
    let source = f.source.clone();
    let input = crate::hosts::WorkerInputRequest {
        kind: crate::hosts::WorkerInputKind::User,
        content: queued_text.into(),
        source: None,
        submission_request_id: Some(queued_id.into()),
        segments: None,
    };
    let accepted = tokio::task::spawn_blocking(move || runtime.send_input(&source, input))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        accepted.disposition,
        crate::hosts::WorkerInputDisposition::Accepted,
        "{accepted:?}"
    );
    let response = f.remove().await;
    assert_eq!(
        response.status,
        StatusCode::CONFLICT.as_u16(),
        "{}",
        response.body
    );
    assert_eq!(
        serde_json::from_str::<Value>(&response.body).unwrap()["code"],
        "worker_not_stopped"
    );
    assert!(
        f.api()
            .config_store
            .recover_worker_removal_execution(TEST_WORKSPACE_ID, &f.source)
            .unwrap()
            .is_none()
    );
    assert!(
        f.api().runtime.worker(&f.source).unwrap().state != "stopped",
        "Remove is not an implicit Stop"
    );
    assert!(f.root().exists());
    f.model.source_release.send_replace(true);
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let state = f.api().runtime.worker(&f.source).unwrap();
            if state.worker_state.as_ref().is_some_and(|state| {
                state.last_finished_submission_request_id.as_deref() == Some(queued_id)
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the accepted queue reaches actual original execution without another Submit");
    assert!(
        f.model
            .source_requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request
                .items
                .iter()
                .filter_map(Item::as_text)
                .any(|text| text.contains(queued_text))),
        "queue acceptance is checked against actual model supply, not a call count"
    );
    let originals = session_store::WorkerSessionStore::open_read_only(f.root()).unwrap();
    let session = f.attr.session_id.parse().unwrap();
    let persisted = originals
        .list_segments(session)
        .unwrap()
        .into_iter()
        .flat_map(|segment| originals.read_all_read_only(session, segment).unwrap())
        .collect::<Vec<_>>();
    let persisted = serde_json::to_string(&persisted).unwrap();
    assert!(persisted.contains(queued_id) && persisted.contains(queued_text));
    let runtime = f.api().runtime.clone();
    let source = f.source.clone();
    let stop_result = tokio::task::spawn_blocking(move || {
        runtime.stop_worker(&source, WorkerLifecycleRequest::default())
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        stop_result.state,
        crate::hosts::InternalWorkerOperationState::Accepted,
        "Stop must confirm actual shutdown before removal, not merely return an HTTP result: {stop_result:?}"
    );
    assert_eq!(f.api().runtime.worker(&f.source).unwrap().state, "stopped");
    let response = f.remove().await;
    assert_eq!(
        response.status,
        StatusCode::OK.as_u16(),
        "{}",
        response.body
    );
    f.wait_for_input().await;
    f.wait_removed().await;
    f.assert_removed_with_pending_recording();
    let rejected = f.api().runtime.send_input(
        &f.source,
        crate::hosts::WorkerInputRequest {
            kind: crate::hosts::WorkerInputKind::User,
            content: "must-not-enter-after-freeze".into(),
            source: None,
            submission_request_id: Some("after-frozen-original".into()),
            segments: None,
        },
    );
    assert!(
        !matches!(&rejected, Ok(result) if result.disposition == crate::hosts::WorkerInputDisposition::Accepted),
        "frozen original input must be rejected: {rejected:?}"
    );
    f.model.set(ModelAction::Record);
    f.wait_recorded().await;
    let delivered = f
        .model
        .inputs
        .lock()
        .unwrap()
        .iter()
        .flat_map(|(_, input)| input.capture.entries.iter())
        .filter_map(|entry| match &entry.body {
            session_store::capture::CaptureBody::CanonicalPublic { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    assert!(delivered.contains("pending-original-model-turn") && delivered.contains(queued_text));
    assert!(!delivered.contains("must-not-enter-after-freeze"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn explicit_source_erasure_uses_permanent_purge_without_a_recording_wait_or_archive() {
    let f = RemovalFixture::new().await;
    f.seed_active(vec![RemovalFixture::history(
        "erase-original",
        "explicitly erased, not silently recorded or archived",
    )]);
    f.api()
        .erase_subjektiv_journal_session(&f.attr.session_id, "explicit-source-erase-before-remove")
        .unwrap();
    let store = open_subjektiv_store(f.api()).unwrap();
    let host = JournalHostContext::validated_session(&store, f.attr.clone()).unwrap();
    assert!(store.journal_session_erased(&host).unwrap());
    assert!(!f.root().exists());
    // Replay while the physical source owner still exists. After Remove its
    // absence is expected; permanent erasure is proved by the retained fence.
    f.api()
        .erase_subjektiv_journal_session(&f.attr.session_id, "explicit-source-erase-before-remove")
        .unwrap();
    let response = f.remove().await;
    assert_eq!(
        response.status,
        StatusCode::OK.as_u16(),
        "{}",
        response.body
    );
    let completed = f.prepared();
    assert_eq!(
        completed.plan.session_disposition,
        worker_runtime::retention::SessionDisposition::Purge
    );
    assert!(completed.plan.archive_id.is_none());
    assert!(
        f.api()
            .config_store
            .get_journal_removal(&completed)
            .unwrap()
            .is_none()
    );
    assert!(
        f.api()
            .config_store
            .worker_session_archive(TEST_WORKSPACE_ID, &f.source, &f.attr.session_id)
            .unwrap()
            .is_none()
    );
    assert!(f.model.inputs.lock().unwrap().is_empty());
    assert!(
        f.api()
            .store
            .list_backend_journal_jobs(TEST_WORKSPACE_ID, &f.attr.session_id, None, 100)
            .unwrap()
            .is_empty()
    );
    assert!(
        subjektiv_journal::bind_host_session(f.api(), f.attr.clone()).is_err(),
        "permanent erase cannot regenerate the body"
    );
    assert!(store.journal_session_erased(&host).unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn nonjournal_stopped_real_worker_removes_without_journal_scope_or_recording_wait() {
    let f = RemovalFixture::new().await;
    let Json(created) = create_workspace_worker_as_owner(
        State(f.api().clone()),
        HeaderMap::new(),
        Json(CreateWorkspaceWorkerRequest {
            runtime_id: EMBEDDED_WORKER_RUNTIME_ID.into(),
            display_name: "plain-nonjournal-source".into(),
            singleton_key: None,
            profile: Some("builtin:companion".into()),
            ticket_assignment: None,
            initial_submit: vec![],
            workdir_attachments: vec![],
            feature_connections: server_api::WorkspaceWorkerFeatureConnectionsRequest::default(),
            control_operation_id: Some(uuid::Uuid::now_v7().to_string()),
            authorization: server_api::launch_policy::WorkerCreationAuthorization::Assignment {
                assignment: None,
            },
        }),
    )
    .await
    .unwrap();
    let plain = RuntimeWorkerRef::new(created.runtime_id, created.worker_id);
    let runtime = f.api().runtime.clone();
    let stopped = plain.clone();
    tokio::task::spawn_blocking(move || {
        runtime.stop_worker(&stopped, WorkerLifecycleRequest::default())
    })
    .await
    .unwrap()
    .unwrap();
    let response = WorkerRemovalService::new(f.api())
        .execute_target_removal(
            f.api().runtime.as_ref(),
            &plain,
            "plain Worker does not need Journal completion",
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        response.status,
        StatusCode::OK.as_u16(),
        "{}",
        response.body
    );
    let completed = f
        .api()
        .config_store
        .recover_worker_removal_execution(TEST_WORKSPACE_ID, &plain)
        .unwrap()
        .unwrap();
    assert!(
        f.api()
            .config_store
            .get_journal_removal(&completed)
            .unwrap()
            .is_none()
    );
    assert!(
        f.api()
            .store
            .get_worker_registry(TEST_WORKSPACE_ID, &plain)
            .unwrap()
            .is_none()
    );
    assert!(f.model.inputs.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn partial_checkpoint_then_real_attempt_stop_recovers_same_input_after_source_removal() {
    let f = RemovalFixture::new().await;
    f.seed_active(vec![
        RemovalFixture::history(
            "partial-first",
            "first original gets a receipt-backed partial checkpoint",
        ),
        RemovalFixture::history(
            "partial-remaining",
            "the same immutable Job still owns these remaining original bytes",
        ),
    ]);
    let response = f.remove().await;
    assert_eq!(
        response.status,
        StatusCode::OK.as_u16(),
        "{}",
        response.body
    );
    f.wait_for_input().await;
    f.wait_removed().await;
    f.assert_removed_with_pending_recording();
    let removal = f.prepared();
    let jobs = f
        .api()
        .store
        .list_backend_journal_jobs(TEST_WORKSPACE_ID, &f.attr.session_id, None, 100)
        .unwrap();
    assert_eq!(jobs.len(), 1);
    let original_job = jobs[0].clone();
    let first = f
        .api()
        .store
        .get_backend_job_attempt(
            TEST_WORKSPACE_ID,
            &original_job.request.job_id,
            &crate::backend_job::attempt_id(
                &original_job.request.job_id,
                original_job.current_attempt,
            ),
        )
        .unwrap()
        .unwrap();
    let old_worker = first.worker.as_ref().unwrap().clone();
    let store = open_subjektiv_store(f.api()).unwrap();
    let host = JournalHostContext::validated_session(&store, f.attr.clone()).unwrap();
    let journal_id = store.journal_for_session(&host).unwrap().unwrap();
    // The producer has been released. Inspect only the exact saved attempt's
    // immutable snapshot; a historical Session cannot reacquire writer authority.
    assert!(matches!(
        store.active_journal_generation(&host, &journal_id),
        Err(subjektiv::journal::JournalError::PermissionDenied(_))
    ));
    let first_generation = store
        .retained_journal_generation(
            &host,
            &journal_id,
            &subjektiv::api::JobAttemptBinding {
                job_id: original_job.request.job_id.clone(),
                attempt_id: first.attempt_id.clone(),
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(first_generation.selected_ranges.len(), 2);
    let checkpoint_returned = f.model.partial_checkpoint.notified();
    f.model.set(ModelAction::PartialThenHold);
    tokio::time::timeout(Duration::from_secs(20), checkpoint_returned)
        .await
        .expect("the real first attempt saved a receipt-backed partial checkpoint");
    f.model.set(ModelAction::Hold);
    let record_id = format!("record:{}", first_generation.id);
    let checkpoint_id = format!("checkpoint:{}", first_generation.id);
    let saved_record = store
        .query_journal_operation(&host, &journal_id, &record_id)
        .unwrap()
        .unwrap();
    let saved_checkpoint = store
        .query_journal_operation(&host, &journal_id, &checkpoint_id)
        .unwrap()
        .unwrap();
    let partial = store.read_journal(&host, &journal_id).unwrap();
    assert_eq!((partial.process_order, partial.process_byte), (1, 0));
    assert!(store.journal_retention_required(&host).unwrap());
    assert!(
        subjektiv_journal::finish(f.api(), &old_worker).is_err(),
        "partial checkpoint cannot complete this Job"
    );
    f.assert_removed_with_pending_recording();
    // Stop the exact real Journal Controller. T841 must obtain its genuine Stop,
    // Host-drain and cleanup evidence before it can fence and replace this attempt.
    // This tests supported logical cancellation, not arbitrary OS process death.
    let runtime = f.api().runtime.clone();
    let stopped = old_worker.clone();
    let stop = tokio::task::spawn_blocking(move || {
        runtime.stop_worker(&stopped, WorkerLifecycleRequest::default())
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        stop.state,
        InternalWorkerOperationState::Accepted,
        "{stop:?}"
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        while f
            .api()
            .runtime
            .backend_job_termination(&old_worker)
            .unwrap()
            .is_none()
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("exact real Controller/Feature termination receipt after Stop");
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let changed = f.model.changed.notified();
            if f.model
                .attempt_inputs
                .lock()
                .unwrap()
                .iter()
                .any(|(id, _)| id != &first.attempt_id)
            {
                break;
            }
            changed.await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "same-Job recovery stalled: {:?}",
            f.api()
                .store
                .list_backend_job_attempts(TEST_WORKSPACE_ID, &original_job.request.job_id)
                .unwrap()
        )
    });
    let retry_job = f
        .api()
        .store
        .get_backend_job(TEST_WORKSPACE_ID, &original_job.request.job_id)
        .unwrap()
        .unwrap();
    assert_eq!(retry_job.request.job_id, original_job.request.job_id);
    assert_eq!(retry_job.current_attempt, original_job.current_attempt + 1);
    assert_eq!(retry_job.input_digest, original_job.input_digest);
    assert!(
        retry_job.load_executable_request().unwrap().input
            == original_job.load_executable_request().unwrap().input,
        "retry must preserve the complete saved immutable input (body omitted)"
    );
    let retry_generation = store
        .retained_journal_generation(
            &host,
            &journal_id,
            &subjektiv::api::JobAttemptBinding {
                job_id: retry_job.request.job_id.clone(),
                attempt_id: crate::backend_job::attempt_id(
                    &retry_job.request.job_id,
                    retry_job.current_attempt,
                ),
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(retry_generation.job.job_id, first_generation.job.job_id);
    assert_ne!(
        retry_generation.job.attempt_id,
        first_generation.job.attempt_id
    );
    assert_eq!(
        retry_generation.selected_ranges,
        first_generation.selected_ranges
    );
    assert_eq!(
        retry_generation.initial_journal,
        first_generation.initial_journal
    );
    let rendered_attempts = f.model.attempt_inputs.lock().unwrap().clone();
    assert_eq!(rendered_attempts.len(), 2);
    assert!(
        rendered_attempts[0].1 == rendered_attempts[1].1,
        "compare actual model-rendered immutable bodies, not dispatch counts (body omitted)"
    );
    assert_eq!(
        store
            .query_journal_operation(&host, &journal_id, &record_id)
            .unwrap(),
        Some(saved_record)
    );
    assert_eq!(
        store
            .query_journal_operation(&host, &journal_id, &checkpoint_id)
            .unwrap(),
        Some(saved_checkpoint)
    );
    assert_eq!(
        (
            store
                .read_journal(&host, &journal_id)
                .unwrap()
                .process_order,
            store.read_journal(&host, &journal_id).unwrap().process_byte
        ),
        (1, 0)
    );
    assert!(
        subjektiv_journal::execute(f.api(), &old_worker, JournalJobRequest::ReadState).is_err()
    );
    let ended_first = f
        .api()
        .store
        .get_backend_job_attempt(
            TEST_WORKSPACE_ID,
            &original_job.request.job_id,
            &first.attempt_id,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        ended_first.worker_cleanup_state,
        Some(BackendJobWorkerCleanupState::Completed)
    );
    f.api()
        .require_backend_job_attempt_effects_ended(&ended_first)
        .unwrap();
    assert!(store.journal_retention_required(&host).unwrap());
    f.assert_removed_with_pending_recording();
    assert_eq!(f.prepared().plan.operation_id, removal.plan.operation_id);
    f.model.set(ModelAction::Record);
    // The retry consumes only the unreflected suffix from its actual ReadState.
    // Source removal is already complete; wait for independent recording/teardown.
    f.wait_recorded().await;
    let complete = store.read_journal(&host, &journal_id).unwrap();
    assert_eq!((complete.process_order, complete.process_byte), (2, 0));
    let mut covered = store
        .read_journal_operation_coverage(&host, &journal_id, &record_id, 0, 32)
        .unwrap()
        .items;
    covered.extend(
        store
            .read_journal_operation_coverage(
                &host,
                &journal_id,
                &format!("record:{}", retry_generation.id),
                0,
                32,
            )
            .unwrap()
            .items,
    );
    covered.sort_by_key(|range| (range.source.order, range.bytes.start));
    assert_eq!(
        covered, first_generation.selected_ranges,
        "saved first/retry receipts cover every immutable byte exactly once"
    );
    let completed_jobs = f
        .api()
        .store
        .list_backend_journal_jobs(TEST_WORKSPACE_ID, &f.attr.session_id, None, 100)
        .unwrap();
    assert_eq!(completed_jobs.len(), 1);
    assert_eq!(completed_jobs[0].state, BackendJobState::Completed);
    assert_eq!(completed_jobs[0].input_digest, original_job.input_digest);
    assert_job_teardown(f.api(), &completed_jobs[0]);
    assert!(!store.journal_retention_required(&host).unwrap());
    assert_eq!(f.prepared().plan.operation_id, removal.plan.operation_id);
    assert_eq!(
        f.prepared().plan.input_fingerprint,
        removal.plan.input_fingerprint
    );
}
