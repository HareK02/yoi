async fn attention_test_api() -> (
    tempfile::TempDir,
    WorkspaceApi,
    Arc<DeterministicExecutionBackend>,
) {
    let dir = tempfile::tempdir().unwrap();
    init_clean_git_workspace(dir.path());
    let (api, execution) = test_api_with_recording_backend(dir.path()).await;
    attention_assign_ticket(&api, "Attention backlog");
    (dir, api, execution)
}

fn attention_assign_ticket(api: &WorkspaceApi, title: &str) -> ticket::TicketRef {
    let mut input = ticket::NewTicket::new(title);
    input.workflow_state = Some(TicketState::Open);
    set_test_ticket_target(&mut input, "test-repository", "HEAD");
    let ticket = browser_ticket_backend(api).unwrap().create(input).unwrap();
    assign_test_orchestrator(api, &ticket.id);
    ticket
}

fn attention_pending(api: &WorkspaceApi) -> Vec<crate::store::TicketAssignmentDelivery> {
    api.store
        .list_pending_ticket_assignment_deliveries(TEST_WORKSPACE_ID, 100)
        .unwrap()
}

async fn start_attention_test_orchestrator(
    api: &WorkspaceApi,
) -> BrowserWorkspaceOrchestratorResponse {
    scoped_start_workspace_orchestrator(
        State(api.clone()),
        AxumPath(ScopedWorkspacePath {
            workspace_id: TEST_WORKSPACE_ID.to_string(),
        }),
    )
    .await
    .unwrap()
    .0
}

#[tokio::test]
async fn orchestrator_attention_parallel_start_and_idle_observation_send_once() {
    let (_dir, api, execution) = attention_test_api().await;
    let reentrant_api = api.clone();
    execution.before_input_returns(move |worker| {
        use protocol::subscription::SubscriptionWorkerState as S;
        // The callback is inside Runtime send_input. Neither initial Idle nor
        // a real turn end may deadlock or issue another in-flight delivery.
        maybe_dispatch_orchestrator_turn_end(
            &reentrant_api,
            &worker.worker_id.to_string(),
            None,
            S::Idle,
        );
        maybe_dispatch_orchestrator_turn_end(
            &reentrant_api,
            &worker.worker_id.to_string(),
            Some(S::Running),
            S::Idle,
        );
    });
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let results = std::thread::scope(|scope| {
        let joins = (0..8)
            .map(|_| {
                let api = api.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    barrier.wait();
                    futures::executor::block_on(start_attention_test_orchestrator(&api))
                })
            })
            .collect::<Vec<_>>();
        joins
            .into_iter()
            .map(|j| j.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(
        results
            .iter()
            .filter(|r| r.disposition == "created")
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|r| r.disposition == "existing")
            .count(),
        7
    );
    assert_eq!(execution.take_inputs().len(), 1);
    assert_eq!(execution.take_input_request_ids().len(), 1);
    assert_eq!(
        start_attention_test_orchestrator(&api).await.disposition,
        "existing"
    );
    assert!(execution.take_inputs().is_empty());
}

#[tokio::test]
async fn orchestrator_attention_barrier_callers_share_unsent_reservation() {
    let (_dir, api, execution) = attention_test_api().await;
    let started = start_attention_test_orchestrator(&api).await;
    let worker = RuntimeWorkerRef::new(
        EMBEDDED_WORKER_RUNTIME_ID,
        started.worker.unwrap().worker_id,
    );
    let state = orchestrator_attention_state(&api, &worker);
    state.lock().unwrap().accepted = None;
    attention_assign_ticket(&api, "Unreceived concurrent input");
    execution.take_inputs();
    execution.take_input_request_ids();
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    execution.before_input_returns(move |_| {
        entered_tx.send(()).unwrap();
        release_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
    });
    std::thread::scope(|scope| {
        let joins = (0..8)
            .map(|_| {
                let api = api.clone();
                let worker = worker.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    barrier.wait();
                    dispatch_orchestrator_assignment_attention(&api, &worker);
                })
            })
            .collect::<Vec<_>>();
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(state.lock().unwrap().sending);
        assert!(state.lock().unwrap().accepted.is_none());
        // The success record does not exist yet. A ninth caller must still
        // return immediately rather than compare/send twice or wait on Runtime.
        dispatch_orchestrator_assignment_attention(&api, &worker);
        release_tx.send(()).unwrap();
        for join in joins {
            join.join().unwrap();
        }
    });
    assert_eq!(execution.take_inputs().len(), 1);
    assert_eq!(execution.take_input_request_ids().len(), 1);
    assert!(!state.lock().unwrap().sending);
    assert!(state.lock().unwrap().accepted.is_some());
}

#[tokio::test]
async fn orchestrator_attention_input_rechecks_reserved_noop_before_and_after_comparison() {
    // This handshake holds a reservation across the real guarded Restore path
    // (including durable intent/config/Runtime writes). Its release deadline is
    // a deadlock watchdog, not a 5s Restore performance contract: the parallel
    // normal run exceeded that old bound before it could release the caller.
    let restore_watchdog = std::time::Duration::from_secs(15);
    for pause_before_comparison in [true, false] {
        // This case only needs assigned Ticket metadata, not a checkout. Keep
        // the real API/storage and configured target, but skip Git init/commit.
        let _dir = tempfile::tempdir().unwrap();
        let (api, execution) = test_api_with_recording_backend(_dir.path()).await;
        attention_assign_ticket(&api, "Attention backlog");
        let started = start_attention_test_orchestrator(&api).await;
        let worker = RuntimeWorkerRef::new(
            EMBEDDED_WORKER_RUNTIME_ID,
            started.worker.unwrap().worker_id,
        );
        let state = orchestrator_attention_state(&api, &worker);
        let acknowledged = state.lock().unwrap().accepted.as_ref().unwrap().clone();
        assert!(attention_pending(&api).is_empty());
        execution.take_inputs();
        execution.take_input_request_ids();
        api.runtime
            .stop_worker(
                &worker,
                WorkerLifecycleRequest {
                    reason: None,
                    ticket_assignment: None,
                },
            )
            .unwrap();
        execution.accept_restores();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let api_for_caller = api.clone();
            let worker_for_caller = worker.clone();
            let caller = scope.spawn(move || {
                let mut checks = 0;
                with_orchestrator_attention_reservation(
                    &api_for_caller,
                    &worker_for_caller,
                    true,
                    |state, pending| {
                        if checks == 0 && pause_before_comparison {
                            entered_tx.send(()).unwrap();
                            release_rx
                                .recv_timeout(restore_watchdog)
                                .expect("guarded Restore did not release the reserved no-op");
                        }
                        check_orchestrator_assignment_attention(
                            &api_for_caller,
                            &worker_for_caller,
                            state,
                            pending,
                        );
                        if checks == 0 && !pause_before_comparison {
                            // Durable acknowledgement matched: check returned without sending,
                            // but the exact same production reservation still exists.
                            assert_eq!(
                                state.lock().unwrap().accepted.as_ref().unwrap().request_id,
                                acknowledged.request_id
                            );
                            entered_tx.send(()).unwrap();
                            release_rx
                                .recv_timeout(restore_watchdog)
                                .expect("guarded Restore did not release the reserved no-op");
                        }
                        checks += 1;
                        assert!(attention_pending(&api_for_caller).is_empty());
                    },
                );
                assert_eq!(checks, 2, "restore must recheck the reserved no-op");
            });
            entered_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            assert!(state.lock().unwrap().sending);
            let restored = futures::executor::block_on(start_attention_test_orchestrator(&api));
            assert_eq!(restored.disposition, "restored");
            let deferred = state.lock().unwrap().input_recheck;
            let old_retired = state.lock().unwrap().accepted.is_none();
            let inputs_before_release = execution.take_inputs();
            release_tx.send(()).unwrap();
            caller.join().unwrap();
            assert!(deferred);
            assert!(old_retired);
            assert!(inputs_before_release.is_empty());
        });
        assert!(execution.take_inputs().is_empty());
        assert!(execution.take_input_request_ids().is_empty());
        assert!(attention_pending(&api).is_empty());
        let state = state.lock().unwrap();
        assert!(state.accepted.is_none());
        assert!(!state.sending);
        assert!(!state.input_recheck);
    }
}

#[tokio::test]
async fn orchestrator_attention_restore_keeps_pending_unknown_delivery_and_id() {
    let (_dir, api, execution) = attention_test_api().await;
    execution.reject_inputs("unknown acceptance response");
    let started = start_attention_test_orchestrator(&api).await;
    let worker = RuntimeWorkerRef::new(
        EMBEDDED_WORKER_RUNTIME_ID,
        started.worker.unwrap().worker_id,
    );
    let state = orchestrator_attention_state(&api, &worker);
    let pending = state.lock().unwrap().pending.clone().unwrap();
    execution.take_inputs();
    execution.take_input_request_ids();
    api.runtime
        .stop_worker(
            &worker,
            WorkerLifecycleRequest {
                reason: None,
                ticket_assignment: None,
            },
        )
        .unwrap();
    execution.accept_restores();
    *execution.input_failure.lock().unwrap() = None;
    let restored = start_attention_test_orchestrator(&api).await;
    assert_eq!(restored.disposition, "restored");
    let attempts = execution.take_inputs();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].1, pending.content);
    assert_eq!(
        execution.take_input_request_ids(),
        vec![Some(pending.request_id.clone())]
    );
    assert!(state.lock().unwrap().pending.is_none());
    assert_eq!(
        state.lock().unwrap().accepted.as_ref().unwrap().request_id,
        pending.request_id
    );
}

#[tokio::test]
async fn orchestrator_attention_unknown_response_retry_keeps_id_and_payload() {
    let (_dir, api, execution) = attention_test_api().await;
    execution.reject_inputs("accepted response lost (unknown transport outcome)");
    let started = start_attention_test_orchestrator(&api).await;
    let worker = RuntimeWorkerRef::new(
        EMBEDDED_WORKER_RUNTIME_ID,
        started.worker.unwrap().worker_id,
    );
    let state = orchestrator_attention_state(&api, &worker);
    assert!(state.lock().unwrap().accepted.is_none());
    let pending = state.lock().unwrap().pending.clone().unwrap();
    let attempts = execution.take_inputs();
    let ids = execution.take_input_request_ids();
    assert_eq!(attempts.len(), 2);
    assert_eq!(ids, vec![Some(pending.request_id.clone()); 2]);
    // Change the backlog before retry: uncertain acceptance must never reuse
    // the ID for a newly rendered payload.
    attention_assign_ticket(&api, "Changed backlog");
    *execution.input_failure.lock().unwrap() = None;
    assert_eq!(
        start_attention_test_orchestrator(&api).await.disposition,
        "existing"
    );
    let retry = execution.take_inputs();
    assert_eq!(
        retry.len(),
        2,
        "acknowledged retry must also drain the changed backlog"
    );
    assert_eq!(retry[0].1, pending.content);
    assert!(retry[1].1.contains("Changed backlog"));
    let ids = execution.take_input_request_ids();
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], Some(pending.request_id));
    assert_ne!(ids[1], ids[0]);
    assert!(state.lock().unwrap().pending.is_none());
    assert!(state.lock().unwrap().accepted.is_some());
    assert!(attention_pending(&api).is_empty());
    use protocol::subscription::SubscriptionWorkerState as S;
    maybe_dispatch_orchestrator_turn_end(&api, &worker.worker_id, Some(S::Running), S::Idle);
    assert!(execution.take_inputs().is_empty());
}

#[tokio::test]
async fn orchestrator_attention_subscription_snapshots_only_establish_baselines() {
    use crate::runtime_subscription::BrokerSubscriptionEvent as B;
    use protocol::subscription::{
        SubscriptionEventPayload, SubscriptionSnapshot, SubscriptionWorker,
        SubscriptionWorkerState as S,
    };
    let (_dir, api, execution) = attention_test_api().await;
    let started = start_attention_test_orchestrator(&api).await;
    let worker = RuntimeWorkerRef::new(
        EMBEDDED_WORKER_RUNTIME_ID,
        started.worker.unwrap().worker_id,
    );
    let attention = orchestrator_attention_state(&api, &worker);
    attention.lock().unwrap().accepted = None;
    execution.take_inputs();
    attention_assign_ticket(&api, "Unreceived after reconnect");
    let summary = |state: S| -> SubscriptionWorker {
        serde_json::from_value(serde_json::json!({
            "worker_id": worker.worker_id, "state": state,
        }))
        .unwrap()
    };
    let snapshot = |state| B::Snapshot {
        snapshot: SubscriptionSnapshot::Workers {
            workers: vec![summary(state)],
        },
    };
    let event = |state| B::Event {
        payload: SubscriptionEventPayload::WorkerUpserted {
            worker: summary(state),
        },
    };
    let mut states = HashMap::new();
    assert!(observe_orchestrator_turn_end(
        &api,
        &mut states,
        snapshot(S::Idle)
    ));
    assert!(observe_orchestrator_turn_end(
        &api,
        &mut states,
        event(S::Idle)
    ));
    observe_orchestrator_turn_end(&api, &mut states, event(S::Running));
    // Neither snapshot itself nor an Idle event after reconnect is turn-end evidence.
    observe_orchestrator_turn_end(
        &api,
        &mut states,
        B::Disconnected {
            message: "fixture reconnect".into(),
        },
    );
    observe_orchestrator_turn_end(&api, &mut states, snapshot(S::Idle));
    observe_orchestrator_turn_end(&api, &mut states, event(S::Idle));
    observe_orchestrator_turn_end(&api, &mut states, snapshot(S::Running));
    observe_orchestrator_turn_end(&api, &mut states, snapshot(S::Idle));
    assert!(attention.lock().unwrap().accepted.is_none());
    assert!(execution.take_inputs().is_empty());
    observe_orchestrator_turn_end(&api, &mut states, event(S::Running));
    observe_orchestrator_turn_end(&api, &mut states, event(S::Idle));
    assert_eq!(execution.take_inputs().len(), 1);
}

// Real Controller and canonical Session storage with isolated local config.
// A closed loopback model endpoint prevents any production-provider access;
// notifications commit at the model boundary before that request can fail.
struct AttentionWorkerFactory {
    root: PathBuf,
}

impl AttentionWorkerFactory {
    async fn controller(
        &self,
        worker_id: String,
        restore: bool,
    ) -> std::result::Result<worker_runtime::worker_backend::RuntimeWorkerController, String> {
        let name = format!("attention-{worker_id}");
        let root = self.root.join("workers").join(&worker_id);
        let store = session_store::CombinedStore::new(
            session_store::WorkerSessionStore::new(root.join("session"))
                .map_err(|e| e.to_string())?,
            session_store::WorkerAggregateStore::new(&root, &name).map_err(|e| e.to_string())?,
        );
        let manifest = manifest::WorkerManifest::from_toml(&format!(
            r#"
                [worker]
                name = "{name}"
                pwd = "./"
                [model]
                scheme = "anthropic"
                model_id = "attention-test"
                base_url = "http://127.0.0.1:1"
                auth = {{ kind = "none" }}
                [engine]
                max_tokens = 10
                [scope]
                allow = []
            "#
        ))
        .map_err(|e| e.to_string())?;
        let context = worker::WorkerWorkspaceContext::local_filesystem(None);
        let workspace_client = context.client_handle();
        let loader = worker::PromptCatalogSource::builtins_only();
        let worker = if restore {
            worker::Worker::restore_from_worker_metadata_with_context(
                &name,
                manifest,
                store,
                loader,
                context,
                worker::WorkerFilesystemAuthority::None,
            )
            .await
        } else {
            worker::Worker::from_manifest_with_context(
                manifest,
                store,
                loader,
                context,
                worker::WorkerFilesystemAuthority::None,
            )
            .await
        }
        .map_err(|e| e.to_string())?;
        let started = worker::PreparedWorker::new(
            worker.with_notification_coalesce_delay(std::time::Duration::from_millis(10)),
            worker::WorkerBootstrapLayout::RuntimeManagedRun {
                run_dir: root.join(uuid::Uuid::now_v7().to_string()),
            },
        )
        .start()
        .await
        .map_err(|e| e.to_string())?;
        Ok(worker_runtime::worker_backend::RuntimeWorkerController {
            handle: started.handle,
            shutdown: Arc::new(tokio::sync::Mutex::new(Some(started.shutdown))),
            controller_task: started.controller_task,
            workspace_client,
            restore_streams: None,
        })
    }
}

#[async_trait::async_trait]
impl worker_runtime::worker_backend::RuntimeWorkerFactory for AttentionWorkerFactory {
    async fn spawn_controller(
        &self,
        request: worker_runtime::execution::WorkerExecutionSpawnRequest,
    ) -> std::result::Result<worker_runtime::worker_backend::RuntimeWorkerController, String> {
        self.controller(request.worker_ref.worker_id.to_string(), false)
            .await
    }
    async fn prepare_restore(
        &self,
        request: worker_runtime::execution::WorkerExecutionRestoreRequest,
    ) -> std::result::Result<
        Box<dyn worker_runtime::worker_backend::RuntimeRestoreCandidate>,
        worker_runtime::worker_backend::RuntimeRestoreFailure,
    > {
        Ok(Box::new(AttentionRestoreCandidate {
            inputs: Some((
                AttentionWorkerFactory {
                    root: self.root.clone(),
                },
                request.worker_ref.worker_id.to_string(),
            )),
        }))
    }
}

// Preparation owns only fixture inputs; reconstruction and Controller startup
// remain lazy so discard never needs to shut down a started Worker.
struct AttentionRestoreCandidate {
    inputs: Option<(AttentionWorkerFactory, String)>,
}

#[async_trait::async_trait]
impl worker_runtime::worker_backend::RuntimeRestoreCandidate for AttentionRestoreCandidate {
    fn initial_state(&self) -> protocol::WorkerStateSnapshot {
        protocol::WorkerStateSnapshot::initial()
    }

    async fn activate(
        &mut self,
    ) -> std::result::Result<
        worker_runtime::worker_backend::RuntimeWorkerController,
        worker_runtime::worker_backend::RuntimeRestoreFailure,
    > {
        let (factory, worker_id) = self.inputs.take().ok_or_else(|| {
            worker_runtime::worker_backend::RuntimeRestoreFailure::refused(
                worker_runtime::worker_backend::RuntimeRestoreStage::Controller,
                "attention fixture candidate already consumed",
            )
        })?;
        factory.controller(worker_id, true).await.map_err(|_| {
            worker_runtime::worker_backend::RuntimeRestoreFailure::unproven(
                worker_runtime::worker_backend::RuntimeRestoreStage::Reconstruction,
                "attention fixture reconstruction failed",
            )
        })
    }

    async fn discard(
        &mut self,
    ) -> std::result::Result<(), worker_runtime::worker_backend::RuntimeRestoreCleanupFailure> {
        self.inputs.take();
        Ok(())
    }
}

use worker_runtime::execution as attention_execution;

// Fault injection occurs AFTER the real Controller durably accepted input.
struct AttentionResponseLossBackend {
    inner: Arc<dyn attention_execution::WorkerExecutionBackend>,
    lose_next: std::sync::atomic::AtomicBool,
    attempts: Mutex<Vec<String>>,
}

impl attention_execution::WorkerExecutionBackend for AttentionResponseLossBackend {
    fn backend_id(&self) -> &str {
        self.inner.backend_id()
    }
    fn spawn_worker(
        &self,
        r: attention_execution::WorkerExecutionSpawnRequest,
    ) -> attention_execution::WorkerExecutionSpawnResult {
        self.inner.spawn_worker(r)
    }
    fn preflight_restore(
        &self,
        r: &attention_execution::WorkerExecutionRestoreRequest,
    ) -> std::result::Result<(), attention_execution::WorkerExecutionResult> {
        self.inner.preflight_restore(r)
    }
    fn reconcile_restore(
        &self,
        r: attention_execution::WorkerExecutionRestoreRequest,
    ) -> attention_execution::WorkerExecutionSpawnResult {
        self.inner.reconcile_restore(r)
    }
    fn restore_worker(
        &self,
        r: attention_execution::WorkerExecutionRestoreRequest,
    ) -> attention_execution::WorkerExecutionSpawnResult {
        self.inner.restore_worker(r)
    }
    fn activate_restored_worker(
        &self,
        id: attention_execution::WorkerLifecycleOperationId,
        worker_ref: &worker_runtime::identity::WorkerRef,
    ) -> std::result::Result<(), String> {
        self.inner.activate_restored_worker(id, worker_ref)
    }
    fn dispatch_input(
        &self,
        worker_ref: &worker_runtime::identity::WorkerRef,
        input: worker_runtime::interaction::WorkerInput,
    ) -> attention_execution::WorkerExecutionResult {
        self.attempts
            .lock()
            .unwrap()
            .push(input.submission_request_id.clone().unwrap());
        let result = self.inner.dispatch_input(worker_ref, input);
        assert_eq!(
            result.outcome,
            attention_execution::WorkerExecutionOutcome::Accepted
        );
        if self
            .lose_next
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            attention_execution::WorkerExecutionResult::errored(
                attention_execution::WorkerExecutionOperation::Input,
                "injected loss of accepted response",
            )
        } else {
            result
        }
    }
    fn dispatch_method(
        &self,
        worker_ref: &worker_runtime::identity::WorkerRef,
        method: protocol::Method,
    ) -> attention_execution::WorkerExecutionResult {
        self.inner.dispatch_method(worker_ref, method)
    }
    fn stop_worker_operation(
        &self,
        r: attention_execution::WorkerExecutionStopRequest,
    ) -> attention_execution::WorkerExecutionResult {
        self.inner.stop_worker_operation(r)
    }
    fn worker_snapshot(
        &self,
        worker_ref: &worker_runtime::identity::WorkerRef,
    ) -> Option<protocol::Event> {
        self.inner.worker_snapshot(worker_ref)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn orchestrator_attention_real_worker_restore_keeps_receipt_and_new_input_commits_once() {
    real_attention_worker_start_restore(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn orchestrator_attention_real_worker_lost_response_retry_commits_once() {
    real_attention_worker_start_restore(true).await;
}

async fn real_attention_worker_start_restore(lose_response: bool) {
    use session_store::{LogEntry, Store, WorkerSessionStore};
    let dir = restored_parent_control_tests::connected_restore_tempdir();
    init_clean_git_workspace(dir.path());
    let config = test_server_config(dir.path());
    let store = Arc::new(SqliteWorkspaceStore::open_for_test(&config.database_path).unwrap());
    seed_test_registered_workspace(store.as_ref(), &config)
        .await
        .unwrap();
    // Real Worker Runtime adapter/controller/storage, isolated from profile
    // workspace callbacks and real LLM credentials.
    let execution = Arc::new(
        worker_runtime::worker_backend::WorkerRuntimeExecutionBackend::new(
            AttentionWorkerFactory {
                root: config.embedded_runtime_store_root.clone(),
            },
        )
        .unwrap(),
    );
    let execution = Arc::new(AttentionResponseLossBackend {
        inner: execution,
        lose_next: std::sync::atomic::AtomicBool::new(lose_response),
        attempts: Mutex::new(Vec::new()),
    });
    let api = WorkspaceApi::new_with_execution_backend(config, store, execution.clone())
        .await
        .unwrap();
    seed_test_delivery_binding(&api);
    let mut input = ticket::NewTicket::new("Real notification backlog");
    input.workflow_state = Some(TicketState::Open);
    set_test_ticket_target(&mut input, "test-repository", "HEAD");
    let ticket = browser_ticket_backend(&api).unwrap().create(input).unwrap();
    assign_test_orchestrator(&api, &ticket.id);
    let started = start_attention_test_orchestrator(&api).await;
    let worker = RuntimeWorkerRef::new(
        EMBEDDED_WORKER_RUNTIME_ID,
        started.worker.unwrap().worker_id,
    );
    let session = WorkerSessionStore::open_read_only(
        api.config
            .embedded_runtime_store_root
            .join("workers")
            .join(&worker.worker_id)
            .join("session"),
    )
    .unwrap();
    let entries = || {
        let id = session.session_id().unwrap().unwrap();
        session
            .list_segments(id)
            .unwrap()
            .into_iter()
            .flat_map(|segment| session.read_all_read_only(id, segment).unwrap())
            .collect::<Vec<_>>()
    };
    let receipts = || {
        entries()
            .iter()
            .rev()
            .find_map(|entry| match entry {
                LogEntry::Extension {
                    domain, payload, ..
                } if domain == "worker.pending_activations.v1" => {
                    Some(payload["notification_receipts"].as_array().unwrap().clone())
                }
                LogEntry::AnnotatedSystemItem { extensions, .. } => extensions
                    .iter()
                    .find(|e| e.domain == "worker.pending_activations.v1")
                    .map(|e| {
                        e.payload["notification_receipts"]
                            .as_array()
                            .unwrap()
                            .clone()
                    }),
                _ => None,
            })
            .unwrap()
    };
    let commits = || {
        entries().iter().filter(|entry| matches!(entry,
            LogEntry::AnnotatedSystemItem { entry, .. }
                if matches!(&entry.item, session_store::SystemItem::Notification { message, .. }
                    if message.starts_with("Assigned Tickets have unreceived input:"))
        )).count()
    };
    async fn wait_for_commits<F: Fn() -> usize>(count: F, expected: usize) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let actual = count();
                assert!(actual <= expected, "duplicate canonical notifications");
                if actual == expected {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("controlled Worker notification commit");
    }
    assert_eq!(receipts().len(), 1);
    wait_for_commits(&commits, 1).await;
    assert_eq!(
        execution.attempts.lock().unwrap().len(),
        if lose_response { 2 } else { 1 }
    );
    let first = orchestrator_attention_state(&api, &worker)
        .lock()
        .unwrap()
        .accepted
        .clone()
        .unwrap();
    let first_id = first.request_id.clone();
    assert_eq!(first.fingerprint, first.input.delivery_id);
    assert!(attention_pending(&api).is_empty());
    use protocol::subscription::SubscriptionWorkerState as S;
    for previous in [
        None,
        Some(S::Idle),
        Some(S::Stopped),
        Some(S::Paused),
        Some(S::Running),
    ] {
        maybe_dispatch_orchestrator_turn_end(&api, &worker.worker_id, previous, S::Idle);
    }
    assert_eq!(
        start_attention_test_orchestrator(&api).await.disposition,
        "existing"
    );
    assert_eq!(receipts().len(), 1);
    api.runtime
        .stop_worker(
            &worker,
            WorkerLifecycleRequest {
                reason: None,
                ticket_assignment: None,
            },
        )
        .unwrap();
    let restored = start_attention_test_orchestrator(&api).await;
    assert_eq!(restored.disposition, "restored");
    assert!(
        orchestrator_attention_state(&api, &worker)
            .lock()
            .unwrap()
            .accepted
            .is_none()
    );
    assert!(attention_pending(&api).is_empty());
    assert_eq!(
        execution.attempts.lock().unwrap().len(),
        if lose_response { 2 } else { 1 }
    );
    assert_eq!(receipts().len(), 1);
    assert_eq!(commits(), 1);
    // Retry directly at the real Controller boundary: the same Worker's
    // persisted receipt survives restore even after server memory is retired.
    let retry = api
        .runtime
        .send_input(
            &worker,
            WorkerInputRequest {
                kind: WorkerInputKind::Notify,
                content: first.content.clone(),
                submission_request_id: Some(first_id.clone()),
                segments: None,
            },
        )
        .unwrap();
    assert_eq!(retry.state, InternalWorkerOperationState::Accepted);
    assert_eq!(receipts().len(), 1);
    assert_eq!(commits(), 1);
    // Only a new external input earns another delivery, not restore itself.
    seed_test_api_token(api.store.as_ref(), "attention-answer");
    let observed = api
        .store
        .list_current_ticket_assignment_tracking(TEST_WORKSPACE_ID, &ticket.id)
        .unwrap()
        .remove(0);
    api.store
        .answer_ticket_assignment(
            TEST_WORKSPACE_ID,
            &ticket.id,
            &ticket::TicketAssignmentAnswer {
                assignment_id: observed.assignment_id,
                expected_last_event_id: observed.last_event_id.unwrap(),
                operation_key: "attention-owner-answer".into(),
                body: "New owner answer".into(),
            },
            &crate::store::TicketOrchestratorAssignmentSourceContext::Account {
                account_id: "account-attention-answer".into(),
                user_id: "user-attention-answer".into(),
            },
            "fixture-account",
        )
        .unwrap();
    maybe_dispatch_orchestrator_turn_end(&api, &worker.worker_id, Some(S::Running), S::Idle);
    let second = orchestrator_attention_state(&api, &worker)
        .lock()
        .unwrap()
        .accepted
        .clone()
        .unwrap();
    assert_eq!(first.input.assignment_id, second.input.assignment_id);
    assert_ne!(first.input.input_event_id, second.input.input_event_id);
    let second_id = second.request_id;
    assert_ne!(first_id, second_id);
    wait_for_commits(&commits, 2).await;
    let attempts = execution.attempts.lock().unwrap();
    assert_eq!(attempts.len(), if lose_response { 4 } else { 3 });
    assert_eq!(
        attempts.iter().filter(|id| **id == first_id).count(),
        if lose_response { 3 } else { 2 }
    );
    assert_eq!(attempts.iter().filter(|id| **id == second_id).count(), 1);
    drop(attempts);
    let receipts = receipts();
    assert_eq!(receipts.len(), 2);
    for id in [first_id, second_id] {
        assert_eq!(
            receipts
                .iter()
                .filter(|r| r["notification_request_id"] == id)
                .count(),
            1
        );
    }
    // Stop only this isolated test fixture; no live dogfooding Worker is touched.
    api.runtime
        .stop_worker(
            &worker,
            WorkerLifecycleRequest {
                reason: None,
                ticket_assignment: None,
            },
        )
        .unwrap();
}

#[tokio::test]
async fn orchestrator_attention_restore_and_replacement_do_not_resend_acknowledged_input() {
    let (_dir, api, execution) = attention_test_api().await;
    let started = start_attention_test_orchestrator(&api).await;
    let worker = RuntimeWorkerRef::new(
        EMBEDDED_WORKER_RUNTIME_ID,
        started.worker.unwrap().worker_id,
    );
    assert_eq!(execution.take_inputs().len(), 1);
    api.runtime
        .stop_worker(
            &worker,
            WorkerLifecycleRequest {
                reason: None,
                ticket_assignment: None,
            },
        )
        .unwrap();
    execution.accept_restores();
    let restored = start_attention_test_orchestrator(&api).await;
    assert_eq!(restored.disposition, "restored");
    assert_eq!(restored.worker.unwrap().worker_id, worker.worker_id);
    assert!(execution.take_inputs().is_empty());
    assert!(attention_pending(&api).is_empty());
    use protocol::subscription::SubscriptionWorkerState as S;
    maybe_dispatch_orchestrator_turn_end(&api, &worker.worker_id, None, S::Idle);
    assert_eq!(
        start_attention_test_orchestrator(&api).await.disposition,
        "existing"
    );
    assert!(execution.take_inputs().is_empty());
    // This is a fixture Worker, not the running dogfooding Orchestrator.
    api.runtime.delete_worker(&worker).unwrap();
    api.worker_projection
        .publish_ordered(|| {
            api.store
                .delete_worker_registry(TEST_WORKSPACE_ID, &worker)
                .map(|commit| ((), commit))
        })
        .unwrap();
    let replacement = start_attention_test_orchestrator(&api).await;
    let replacement_id = replacement.worker.unwrap().worker_id;
    assert_ne!(replacement_id, worker.worker_id);
    assert!(execution.take_inputs().is_empty());
    assert!(attention_pending(&api).is_empty());
    attention_assign_ticket(&api, "New assignment for replacement");
    let replacement = RuntimeWorkerRef::new(EMBEDDED_WORKER_RUNTIME_ID, replacement_id);
    dispatch_orchestrator_assignment_attention(&api, &replacement);
    let inputs = execution.take_inputs();
    assert_eq!(inputs.len(), 1);
    assert!(inputs[0].1.contains("New assignment for replacement"));
}

#[tokio::test]
async fn orchestrator_attention_close_reopen_does_not_revive_old_input_but_reassignment_delivers() {
    for unknown_response in [false, true] {
        let (_dir, api, execution) = attention_test_api().await;
        if unknown_response {
            execution.reject_inputs("unknown acceptance before closure");
        }
        let started = start_attention_test_orchestrator(&api).await;
        let worker = RuntimeWorkerRef::new(
            EMBEDDED_WORKER_RUNTIME_ID,
            started.worker.unwrap().worker_id,
        );
        let attention = orchestrator_attention_state(&api, &worker);
        let original = {
            let state = attention.lock().unwrap();
            if unknown_response {
                state.pending.clone().unwrap()
            } else {
                state.accepted.clone().unwrap()
            }
        };
        execution.take_inputs();
        execution.take_input_request_ids();
        *execution.input_failure.lock().unwrap() = None;
        let backend = browser_ticket_backend(&api).unwrap();
        for (key, action) in [
            (
                "attention-close",
                ticket::TicketCaseAction::Close {
                    reason: ticket::TicketClosureReason::NotPlanned,
                },
            ),
            ("attention-reopen", ticket::TicketCaseAction::Reopen),
        ] {
            let observed = backend
                .show(original.input.ticket_id.clone().into())
                .unwrap();
            backend
                .update_case(
                    &original.input.ticket_id,
                    ticket::TicketCaseUpdate {
                        expected_content_digest: ticket::ticket_content_digest(&observed),
                        expected_state: observed.meta.workflow_state,
                        expected_closure_operation_key: observed.closure_operation_key,
                        operation_key: key.into(),
                        action,
                        explanation: "Human case decision".into(),
                        references: vec![],
                        author: None,
                    },
                )
                .unwrap();
            dispatch_orchestrator_assignment_attention(&api, &worker);
            assert!(attention_pending(&api).is_empty());
            assert!(
                execution.take_inputs().is_empty(),
                "case history must not revive old assignment input"
            );
            assert!(attention.lock().unwrap().pending.is_none());
        }
        // Even an external comment on the reopened case cannot revive the ended assignment.
        backend
            .add_event(
                original.input.ticket_id.clone().into(),
                ticket::NewTicketEvent::new(
                    ticket::TicketEventKind::Comment,
                    "Historical result annotation",
                ),
            )
            .unwrap();
        dispatch_orchestrator_assignment_attention(&api, &worker);
        assert!(attention_pending(&api).is_empty());
        assert!(execution.take_inputs().is_empty());
        api.store
            .set_current_ticket_role_assignment(
                &TicketRoleAssignmentRecord {
                    workspace_id: TEST_WORKSPACE_ID.into(),
                    ticket_id: original.input.ticket_id.clone(),
                    assignment_id: "attention-reassigned".into(),
                    role: TicketAssignmentRole::Orchestrator,
                    principal: TicketAssignmentPrincipal::WorkspaceAgent {
                        agent_key: "workspace-orchestrator".into(),
                    },
                    assigned_by: "test-user".into(),
                    assigned_at: "2026-09-02T00:00:00Z".into(),
                },
                Some(&original.input.assignment_id),
                "attention-reassign-event",
                "attention-reassign-op",
                true,
            )
            .unwrap();
        dispatch_orchestrator_assignment_attention(&api, &worker);
        assert_eq!(
            execution.take_inputs().len(),
            1,
            "explicit new assignment must deliver"
        );
        let accepted = attention.lock().unwrap().accepted.clone().unwrap();
        assert_ne!(original.input.assignment_id, accepted.input.assignment_id);
        assert_ne!(original.request_id, accepted.request_id);
        assert!(attention_pending(&api).is_empty());
    }
}

#[tokio::test]
async fn orchestrator_running_to_idle_recovers_unreceived_input_without_notification_memory() {
    let (_dir, api, execution) = attention_test_api().await;
    let backend = browser_ticket_backend(&api).unwrap();
    let started = start_attention_test_orchestrator(&api).await;
    assert!(started.online);
    assert_eq!(execution.take_inputs().len(), 1);
    let worker_id = started.worker.unwrap().worker_id;
    let worker = RuntimeWorkerRef::new(EMBEDDED_WORKER_RUNTIME_ID, &worker_id);
    let attention = orchestrator_attention_state(&api, &worker);
    let acknowledged = attention.lock().unwrap().accepted.clone().unwrap();
    assert_eq!(acknowledged.fingerprint, acknowledged.input.delivery_id);
    attention.lock().unwrap().accepted = None;
    use protocol::subscription::SubscriptionWorkerState as S;
    maybe_dispatch_orchestrator_turn_end(&api, &worker_id, Some(S::Running), S::Idle);
    assert!(
        execution.take_inputs().is_empty(),
        "lost memory cannot resend durable acknowledgement"
    );
    assert!(attention.lock().unwrap().accepted.is_none());
    let ticket_ref = attention_assign_ticket(&api, "Recover unreceived work");
    let pending = attention_pending(&api);
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].ticket_id, ticket_ref.id);
    // Snapshot/reconnection and non-turn transitions cannot stand in for startup.
    for previous in [None, Some(S::Idle), Some(S::Stopped), Some(S::Paused)] {
        maybe_dispatch_orchestrator_turn_end(&api, &worker_id, previous, S::Idle);
        assert!(attention.lock().unwrap().accepted.is_none());
        assert!(execution.take_inputs().is_empty());
        assert_eq!(attention_pending(&api).len(), 1);
    }
    maybe_dispatch_orchestrator_turn_end(&api, &worker_id, Some(S::Running), S::Idle);
    let accepted = attention.lock().unwrap().accepted.clone().unwrap();
    assert_eq!(accepted.input.assignment_id, pending[0].assignment_id);
    assert_eq!(accepted.input.input_event_id, pending[0].input_event_id);
    assert_eq!(accepted.fingerprint, accepted.input.delivery_id);
    assert!(attention_pending(&api).is_empty());
    maybe_dispatch_orchestrator_turn_end(&api, &worker_id, Some(S::Running), S::Idle);
    let notifications = execution.take_inputs();
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].0.worker_id.to_string(), worker_id);
    let content = &notifications[0].1;
    assert!(content.starts_with("Assigned Tickets have unreceived input:"));
    assert!(
        content.contains(&format!(
            "- {} — Recover unreceived work",
            ticket_ref.resource_key.as_deref().unwrap()
        )),
        "unexpected notification body: {content:?}"
    );
    assert!(content.contains("Reread the current Ticket state before acting"));
    assert!(!content.contains(TEST_WORKSPACE_ID));
    assert!(!content.contains("bounded"));
    assert!(!content.contains("omitted"));

    let candidates = backend
        .list(ticket::TicketListQuery::states([
            ticket::TicketListState::Open,
        ]))
        .unwrap();
    let mut truncated_candidates = (1..=worker::OrchestratorQueueAttentionContext::MAX_TICKETS + 1)
        .map(|index| {
            let mut candidate = candidates[0].clone();
            candidate.id = format!("opaque-{index}");
            candidate.resource_key = Some(format!("T-{index}"));
            candidate.title = format!("Queued {index}");
            candidate
        })
        .collect::<Vec<_>>();
    let truncated = orchestrator_queue_attention_context(
        TEST_WORKSPACE_ID,
        TEST_WORKSPACE_ID,
        &truncated_candidates,
    )
    .unwrap();
    let rendered = worker::PromptCatalog::builtins_only()
        .unwrap()
        .orchestrator_queue_attention(worker::OrchestratorQueueAttentionPrompt::Server, &truncated)
        .unwrap();
    assert!(rendered.contains("- T-20 — Queued 20"));
    assert!(!rendered.contains("T-21"));
    assert!(rendered.contains("were omitted from this notice: 1"));
    assert!(!rendered.contains("opaque-"));

    truncated_candidates[0].resource_key = None;
    assert_eq!(
        orchestrator_queue_attention_context(
            TEST_WORKSPACE_ID,
            TEST_WORKSPACE_ID,
            &truncated_candidates
        )
        .unwrap_err(),
        "missing_ticket_resource_key"
    );
    assert_eq!(
        orchestrator_queue_attention_context(
            TEST_WORKSPACE_ID,
            "foreign-workspace",
            &truncated_candidates
        )
        .unwrap_err(),
        "foreign_workspace_ticket_projection"
    );
}
