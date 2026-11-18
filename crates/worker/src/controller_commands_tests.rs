//! Component coverage of the real Controller inbox, command ledger, Worker and
//! Session store. Only the model provider is held at an explicit async boundary.
//! Unpublished endpoints allow deterministic delivery/admission fault injection;
//! published endpoints exercise public APIs through state validation and effects.
use super::*;
use futures_util::poll;
use session_store::{CombinedStore, FsStore, FsWorkerStore};
use tempfile::TempDir;

#[derive(Clone)]
struct HeldProvider {
    starts: mpsc::UnboundedSender<()>,
    finish: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl LlmClient for HeldProvider {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        _: agen::llm_client::Request,
    ) -> Result<
        std::pin::Pin<
            Box<
                dyn futures_util::Stream<
                        Item = Result<
                            agen::llm_client::event::Event,
                            agen::llm_client::ClientError,
                        >,
                    > + Send,
            >,
        >,
        agen::llm_client::ClientError,
    > {
        self.starts.send(()).unwrap();
        self.finish.notified().await;
        Ok(Box::pin(futures_util::stream::iter([Ok(
            agen::llm_client::event::Event::Status(agen::llm_client::event::StatusEvent {
                status: agen::llm_client::event::ResponseStatus::Completed,
            }),
        )])))
    }
}

type Initialized = InitializedWorkerController<HeldProvider, CombinedStore<FsStore, FsWorkerStore>>;

async fn initialized(
    temp: &TempDir,
) -> (
    Initialized,
    mpsc::UnboundedReceiver<()>,
    Arc<tokio::sync::Notify>,
) {
    initialized_with_rewrite(temp, None).await
}

async fn initialized_with_rewrite(
    temp: &TempDir,
    rewrite: Option<HeldRewrite>,
) -> (
    Initialized,
    mpsc::UnboundedReceiver<()>,
    Arc<tokio::sync::Notify>,
) {
    let (starts, started) = mpsc::unbounded_channel();
    let finish = Arc::new(tokio::sync::Notify::new());
    let client = HeldProvider {
        starts,
        finish: finish.clone(),
    };
    let mut manifest = manifest::WorkerManifest::from_toml(
        "[worker]\nname='command-test'\n[model]\nscheme='anthropic'\nmodel_id='scripted'\n[engine]\n[scope]\nallow=[]",
    ).unwrap();
    manifest.compaction = Some(manifest::CompactionConfig {
        retained_tokens: 0,
        ..Default::default()
    });
    let engine =
        agen::Engine::<_, agen::state::Mutable, crate::SessionHistoryMetadata>::new_annotated(
            client,
        );
    let store = CombinedStore::new(
        FsStore::new(temp.path().join("sessions")).unwrap(),
        FsWorkerStore::new(temp.path().join("workers")).unwrap(),
    );
    let mut worker = Worker::new(
        manifest,
        engine,
        store,
        crate::WorkerWorkspaceContext::no_workspace(),
        crate::WorkerFilesystemAuthority::None,
        manifest::Scope::empty(),
    )
    .await
    .unwrap();
    worker.set_system_prompt_template(
        crate::SystemPromptTemplate::parse("default", crate::PromptCatalogSource::builtins_only())
            .unwrap(),
    );
    if let Some(rewrite) = rewrite {
        let mut registry = FeatureRegistryBuilder::new();
        registry.add_module(rewrite);
        assert!(!worker.install_features(registry).has_errors());
    }
    (
        WorkerController::initialize(worker, temp.path(), None)
            .await
            .unwrap(),
        started,
        finish,
    )
}

async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("command test deadline")
}

async fn wait_idle(events: &mut broadcast::Receiver<Event>) {
    bounded(async {
        loop {
            if matches!(events.recv().await.unwrap(), Event::WorkerState { snapshot } if snapshot.state == WorkerState::Idle) { break; }
        }
    }).await;
}

async fn shutdown(
    handle: WorkerHandle,
    report: ShutdownReceiver,
    task: tokio::task::JoinHandle<()>,
) {
    let ack = bounded(handle.send_host_command(WorkerCommandKind::Shutdown))
        .await
        .unwrap();
    assert_eq!(ack.disposition, WorkerCommandDisposition::Accepted);
    bounded(report).await.unwrap();
    bounded(task).await.unwrap();
}

#[tokio::test]
async fn queued_host_commands_allocate_only_after_external_dequeue_admission() {
    let temp = TempDir::new().unwrap();
    let _sandbox =
        crate::runtime::worker_allocation::test_util::RuntimeDirSandbox::new(temp.path());
    let (mut endpoint, _, _) = initialized(&temp).await;
    let handle = endpoint.handle.clone();
    let stale_observation = handle.shared_state.snapshot();
    handle
        .send(Method::Pause {
            command: WorkerCommandEnvelope::new(50_000),
        })
        .await
        .unwrap();
    let cancel = handle.send_host_command(WorkerCommandKind::Cancel);
    let compact = handle.send_host_command(WorkerCommandKind::Compact);
    let shutdown = handle.send_host_command(WorkerCommandKind::Shutdown);
    tokio::pin!(cancel, compact, shutdown);
    assert!(poll!(&mut cancel).is_pending());
    assert!(poll!(&mut compact).is_pending());
    assert!(poll!(&mut shutdown).is_pending());
    // Even shared state still has the old ID: enqueue did not issue IDs.
    assert_eq!(handle.shared_state.snapshot(), stale_observation);
    let external = bounded(endpoint.method_rx.recv()).await.unwrap();
    assert!(matches!(external, Method::Pause { command } if command.command_id == 50_000));
    for (kind, expected_id) in [
        (WorkerCommandKind::Cancel, 50_001),
        (WorkerCommandKind::Compact, 50_002),
        (WorkerCommandKind::Shutdown, 50_003),
    ] {
        let method = bounded(endpoint.method_rx.recv()).await.unwrap();
        assert_eq!(
            commands::command_identity(&method),
            Some((WorkerCommandEnvelope::new(expected_id), kind))
        );
        acknowledge_command(
            &handle.working_event_tx,
            &handle.shared_state,
            expected_id,
            kind,
            WorkerCommandDisposition::Accepted,
        );
    }
    assert_eq!(bounded(cancel).await.unwrap().command_id, 50_001);
    assert_eq!(bounded(compact).await.unwrap().command_id, 50_002);
    assert_eq!(bounded(shutdown).await.unwrap().command_id, 50_003);
}

#[tokio::test]
async fn pending_exact_retry_joins_original_reply_and_completed_retry_has_no_second_dispatch() {
    let temp = TempDir::new().unwrap();
    let _sandbox =
        crate::runtime::worker_allocation::test_util::RuntimeDirSandbox::new(temp.path());
    let (mut endpoint, _, _) = initialized(&temp).await;
    let handle = endpoint.handle.clone();
    let method = Method::Cancel {
        command: WorkerCommandEnvelope::new(7),
    };
    let first = handle.send_command(method.clone());
    let retry = handle.send_command(method.clone());
    tokio::pin!(first, retry);
    assert!(poll!(&mut first).is_pending());
    assert!(poll!(&mut retry).is_pending());
    assert!(matches!(
        bounded(endpoint.method_rx.recv()).await,
        Some(Method::Cancel { .. })
    ));
    handle.send(Method::ListWorkers).await.unwrap();
    // The retry is consumed into the existing ledger, not returned to a handler.
    assert!(matches!(
        bounded(endpoint.method_rx.recv()).await,
        Some(Method::ListWorkers)
    ));
    assert!(poll!(&mut retry).is_pending());
    acknowledge_command(
        &handle.working_event_tx,
        &handle.shared_state,
        7,
        WorkerCommandKind::Cancel,
        WorkerCommandDisposition::Accepted,
    );
    let original = bounded(first).await.unwrap();
    assert_eq!(bounded(retry).await.unwrap(), original);
    handle.shared_state.transition(WorkerState::Idle);
    let replay = handle.send_command(method);
    tokio::pin!(replay);
    assert!(poll!(&mut replay).is_pending());
    handle.send(Method::ListWorkers).await.unwrap();
    assert!(matches!(
        bounded(endpoint.method_rx.recv()).await,
        Some(Method::ListWorkers)
    ));
    assert_eq!(bounded(replay).await.unwrap(), original);
}

#[tokio::test]
async fn public_commands_preserve_invalid_retry_conflict_stale_and_host_admission() {
    let temp = TempDir::new().unwrap();
    let _sandbox =
        crate::runtime::worker_allocation::test_util::RuntimeDirSandbox::new(temp.path());
    let (endpoint, mut starts, finish) = initialized(&temp).await;
    let (handle, report, task) = endpoint.publish().await.unwrap();
    let mut events = handle.subscribe();
    let pause = Method::Pause {
        command: WorkerCommandEnvelope::new(900),
    };
    let invalid = bounded(handle.send_command(pause.clone())).await.unwrap();
    assert_eq!(invalid.disposition, WorkerCommandDisposition::InvalidState);
    handle
        .send(Method::submit_text("run-for-retry", "held run"))
        .await
        .unwrap();
    bounded(starts.recv()).await.unwrap();
    assert_eq!(bounded(handle.send_command(pause)).await.unwrap(), invalid);
    assert!(matches!(
        handle.shared_state.snapshot().state,
        WorkerState::Busy(WorkerBusyState::Run(WorkerRunState::Running))
    ));
    for (method, expected) in [
        (
            Method::Cancel {
                command: WorkerCommandEnvelope::new(900),
            },
            WorkerCommandDisposition::Conflict,
        ),
        (
            Method::Cancel {
                command: WorkerCommandEnvelope::new(899),
            },
            WorkerCommandDisposition::StaleCommandId,
        ),
    ] {
        assert_eq!(
            bounded(handle.send_command(method))
                .await
                .unwrap()
                .disposition,
            expected
        );
    }
    let compact = bounded(handle.send_host_command(WorkerCommandKind::Compact))
        .await
        .unwrap();
    assert_eq!(compact.command_id, 901);
    assert_eq!(compact.disposition, WorkerCommandDisposition::InvalidState);
    // Allow normal completion, then verify Cancel's invalid state is correlated,
    // not mistaken for enqueue success or compact's disposition.
    finish.notify_one();
    wait_idle(&mut events).await;
    let cancel = bounded(handle.send_host_command(WorkerCommandKind::Cancel))
        .await
        .unwrap();
    assert_eq!(cancel.command_id, 902);
    assert_eq!(cancel.command, WorkerCommandKind::Cancel);
    assert_eq!(cancel.disposition, WorkerCommandDisposition::InvalidState);
    shutdown(handle, report, task).await;
}

#[tokio::test]
async fn accepted_cancel_retry_after_run_exit_replays_without_new_history_or_state_effect() {
    let temp = TempDir::new().unwrap();
    let _sandbox =
        crate::runtime::worker_allocation::test_util::RuntimeDirSandbox::new(temp.path());
    let (endpoint, mut starts, _) = initialized(&temp).await;
    let (handle, report, task) = endpoint.publish().await.unwrap();
    let mut events = handle.subscribe();
    handle
        .send(Method::submit_text("cancel-run", "held run"))
        .await
        .unwrap();
    bounded(starts.recv()).await.unwrap();
    let accepted = bounded(handle.send_host_command(WorkerCommandKind::Cancel))
        .await
        .unwrap();
    assert_eq!(accepted.disposition, WorkerCommandDisposition::Accepted);
    assert!(matches!(
        accepted.state.state,
        WorkerState::Busy(WorkerBusyState::Run(WorkerRunState::Cancelling))
    ));
    wait_idle(&mut events).await;
    let entries = handle.committed_entries();
    assert!(entries.iter().any(|entry| matches!(entry, LogEntry::Extension { domain, .. } if domain == "worker.pending_activations.v1")));
    let state = handle.shared_state.snapshot();
    let replay = bounded(handle.send_command(Method::Cancel {
        command: WorkerCommandEnvelope::new(accepted.command_id),
    }))
    .await
    .unwrap();
    assert_eq!(replay, accepted);
    assert_eq!(handle.shared_state.snapshot(), state);
    assert_eq!(
        serde_json::to_value(handle.committed_entries()).unwrap(),
        serde_json::to_value(entries).unwrap()
    );
    shutdown(handle, report, task).await;
}

#[tokio::test]
async fn response_loss_distinguishes_not_delivered_unissued_and_issued_unknown() {
    for dequeued in [false, true] {
        let temp = TempDir::new().unwrap();
        let _sandbox =
            crate::runtime::worker_allocation::test_util::RuntimeDirSandbox::new(temp.path());
        let (mut endpoint, _, _) = initialized(&temp).await;
        let handle = endpoint.handle.clone();
        let response = handle.send_host_command(WorkerCommandKind::Cancel);
        tokio::pin!(response);
        assert!(poll!(&mut response).is_pending());
        if dequeued {
            assert!(matches!(
                bounded(endpoint.method_rx.recv()).await,
                Some(Method::Cancel { .. })
            ));
        }
        drop(endpoint);
        assert_eq!(
            bounded(response).await,
            Err(WorkerCommandSendError::AdmissionUnknown {
                command_id: dequeued.then_some(1),
                command: WorkerCommandKind::Cancel,
            })
        );
        assert_eq!(
            bounded(handle.send_host_command(WorkerCommandKind::Cancel)).await,
            Err(WorkerCommandSendError::NotDelivered {
                command_id: None,
                command: WorkerCommandKind::Cancel,
                reason: WorkerCommandNotDeliveredReason::ControllerClosed,
            })
        );
        assert_eq!(
            bounded(handle.send_command(Method::Pause {
                command: WorkerCommandEnvelope::new(42)
            }))
            .await,
            Err(WorkerCommandSendError::NotDelivered {
                command_id: Some(42),
                command: WorkerCommandKind::Pause,
                reason: WorkerCommandNotDeliveredReason::ControllerClosed,
            })
        );
    }
}

#[tokio::test]
async fn unrelated_broadcast_acknowledgements_cannot_satisfy_direct_command_reply() {
    let temp = TempDir::new().unwrap();
    let _sandbox =
        crate::runtime::worker_allocation::test_util::RuntimeDirSandbox::new(temp.path());
    let (mut endpoint, _, _) = initialized(&temp).await;
    let handle = endpoint.handle.clone();
    let response = handle.send_command(Method::Pause {
        command: WorkerCommandEnvelope::new(42),
    });
    tokio::pin!(response);
    assert!(poll!(&mut response).is_pending());
    for _ in 0..1024 {
        let _ = handle.send_event(Event::CommandAcknowledged {
            acknowledgement: WorkerCommandAcknowledgement {
                command_id: 42,
                command: WorkerCommandKind::Pause,
                disposition: WorkerCommandDisposition::Accepted,
                state: handle.shared_state.snapshot(),
            },
        });
    }
    assert!(poll!(&mut response).is_pending());
    assert!(matches!(
        bounded(endpoint.method_rx.recv()).await,
        Some(Method::Pause { .. })
    ));
    acknowledge_command(
        &handle.working_event_tx,
        &handle.shared_state,
        42,
        WorkerCommandKind::Pause,
        WorkerCommandDisposition::InvalidState,
    );
    assert_eq!(
        bounded(response).await.unwrap().disposition,
        WorkerCommandDisposition::InvalidState
    );
}

#[tokio::test]
async fn shutdown_exact_retry_reaches_handler_and_replays_recorded_admission() {
    let temp = TempDir::new().unwrap();
    let _sandbox =
        crate::runtime::worker_allocation::test_util::RuntimeDirSandbox::new(temp.path());
    let (mut endpoint, _, _) = initialized(&temp).await;
    let handle = endpoint.handle.clone();
    for _ in 0..2 {
        let response = handle.send_command(Method::Shutdown {
            command: WorkerCommandEnvelope::new(9),
        });
        tokio::pin!(response);
        assert!(poll!(&mut response).is_pending());
        assert!(matches!(
            bounded(endpoint.method_rx.recv()).await,
            Some(Method::Shutdown { .. })
        ));
        acknowledge_command(
            &handle.working_event_tx,
            &handle.shared_state,
            9,
            WorkerCommandKind::Shutdown,
            WorkerCommandDisposition::Accepted,
        );
        assert_eq!(
            bounded(response).await.unwrap().disposition,
            WorkerCommandDisposition::Accepted
        );
    }
}

#[tokio::test]
async fn release_reservation_fences_both_command_apis_but_not_shutdown() {
    let temp = TempDir::new().unwrap();
    let _sandbox =
        crate::runtime::worker_allocation::test_util::RuntimeDirSandbox::new(temp.path());
    let (endpoint, _, _) = initialized(&temp).await;
    let (handle, report, task) = endpoint.publish().await.unwrap();
    let guard = bounded(handle.reserve_subject_release()).await.unwrap();
    assert_eq!(
        bounded(handle.send_host_command(WorkerCommandKind::Cancel)).await,
        Err(WorkerCommandSendError::NotDelivered {
            command_id: None,
            command: WorkerCommandKind::Cancel,
            reason: WorkerCommandNotDeliveredReason::SubjectReleaseReserved,
        })
    );
    assert_eq!(
        bounded(handle.send_command(Method::Compact {
            command: WorkerCommandEnvelope::new(5)
        }))
        .await,
        Err(WorkerCommandSendError::NotDelivered {
            command_id: Some(5),
            command: WorkerCommandKind::Compact,
            reason: WorkerCommandNotDeliveredReason::SubjectReleaseReserved,
        })
    );
    assert_eq!(handle.shared_state.snapshot().last_command_id, 0);
    shutdown(handle, report, task).await;
    drop(guard);
}

#[derive(Clone)]
struct HeldRewrite {
    started: mpsc::UnboundedSender<()>,
    finish: Arc<tokio::sync::Notify>,
}
impl crate::feature::FeatureModule for HeldRewrite {
    fn descriptor(&self) -> crate::feature::FeatureDescriptor {
        crate::feature::FeatureDescriptor::builtin("command-held-rewrite", "Held rewrite")
            .with_hook(crate::feature::HookDeclaration::new(
                "hold",
                crate::feature::FeatureHookPoint::BeforeSessionRewrite,
            ))
    }
    fn install(
        &self,
        context: &mut crate::feature::FeatureInstallContext<'_>,
    ) -> Result<(), crate::feature::FeatureInstallError> {
        context.hooks().add_before_session_rewrite(
            "hold",
            crate::hook::HookExecutionPolicy::fail_closed(),
            self.clone(),
        )?;
        Ok(())
    }
}
#[async_trait::async_trait]
impl crate::hook::Hook<crate::hook::BeforeSessionRewrite> for HeldRewrite {
    async fn call(
        &self,
        context: &crate::hook::BeforeSessionRewriteContext,
    ) -> Result<crate::hook::BeforeSessionRewriteAction, crate::hook::HookError> {
        if context.kind == crate::hook::SessionRewriteKind::Compact {
            self.started.send(()).unwrap();
            self.finish.notified().await;
            return Ok(crate::hook::BeforeSessionRewriteAction::Deny(
                "end isolated compaction".into(),
            ));
        }
        Ok(crate::hook::BeforeSessionRewriteAction::Continue)
    }
}

#[tokio::test]
async fn compact_admission_precedes_completion_and_deferred_ids_do_not_stale_host_cancel() {
    let temp = TempDir::new().unwrap();
    let _sandbox =
        crate::runtime::worker_allocation::test_util::RuntimeDirSandbox::new(temp.path());
    let (rewrite_started, mut rewrites) = mpsc::unbounded_channel();
    let rewrite_finish = Arc::new(tokio::sync::Notify::new());
    let (endpoint, mut starts, finish) = initialized_with_rewrite(
        &temp,
        Some(HeldRewrite {
            started: rewrite_started,
            finish: rewrite_finish.clone(),
        }),
    )
    .await;
    let (handle, report, task) = endpoint.publish().await.unwrap();
    let mut events = handle.subscribe();
    handle
        .send(Method::submit_text(
            "compact-history",
            "history to compact ".repeat(128),
        ))
        .await
        .unwrap();
    bounded(starts.recv()).await.unwrap();
    finish.notify_one();
    wait_idle(&mut events).await;
    let compact = bounded(handle.send_host_command(WorkerCommandKind::Compact))
        .await
        .unwrap();
    assert_eq!(compact.disposition, WorkerCommandDisposition::Accepted);
    bounded(rewrites.recv()).await.unwrap();
    assert!(matches!(
        compact.state.state,
        WorkerState::Busy(WorkerBusyState::Maintenance(
            WorkerMaintenanceState::Compacting
        ))
    ));
    assert_eq!(handle.shared_state.snapshot().state, compact.state.state);
    let replay = bounded(handle.send_command(Method::Compact {
        command: WorkerCommandEnvelope::new(compact.command_id),
    }))
    .await
    .unwrap();
    assert_eq!(replay, compact);
    let deferred = handle.send_command(Method::Resume {
        command: WorkerCommandEnvelope::new(5_000),
    });
    tokio::pin!(deferred);
    assert!(poll!(&mut deferred).is_pending());
    let cancel = bounded(handle.send_host_command(WorkerCommandKind::Cancel))
        .await
        .unwrap();
    assert_eq!(cancel.command_id, 5_001);
    assert_eq!(cancel.disposition, WorkerCommandDisposition::Accepted);
    // Cancellation admission is not proof the held maintenance has exited.
    assert!(matches!(
        handle.shared_state.snapshot().state,
        WorkerState::Busy(WorkerBusyState::Maintenance(_))
    ));
    assert!(poll!(&mut deferred).is_pending());
    rewrite_finish.notify_one();
    wait_idle(&mut events).await;
    let deferred = bounded(deferred).await.unwrap();
    assert_eq!(deferred.command_id, 5_000);
    assert_eq!(deferred.disposition, WorkerCommandDisposition::InvalidState);
    assert_eq!(
        bounded(handle.send_command(Method::Compact {
            command: WorkerCommandEnvelope::new(compact.command_id)
        }))
        .await
        .unwrap(),
        compact
    );
    assert!(
        rewrites.try_recv().is_err(),
        "retry must not invoke the rewrite twice"
    );
    shutdown(handle.clone(), report, task).await;
}

#[tokio::test]
async fn command_id_exhaustion_and_non_command_methods_do_not_dispatch_fresh_effects() {
    let temp = TempDir::new().unwrap();
    let _sandbox =
        crate::runtime::worker_allocation::test_util::RuntimeDirSandbox::new(temp.path());
    let (mut endpoint, _, _) = initialized(&temp).await;
    let handle = endpoint.handle.clone();
    assert_eq!(
        bounded(handle.send_command(Method::ListWorkers)).await,
        Err(WorkerCommandSendError::NotACommand)
    );
    assert!(endpoint.method_rx.is_empty());
    handle
        .send(Method::Pause {
            command: WorkerCommandEnvelope::new(u64::MAX),
        })
        .await
        .unwrap();
    assert!(matches!(
        bounded(endpoint.method_rx.recv()).await,
        Some(Method::Pause { .. })
    ));
    let host = handle.send_host_command(WorkerCommandKind::Cancel);
    tokio::pin!(host);
    assert!(poll!(&mut host).is_pending());
    handle.send(Method::ListWorkers).await.unwrap();
    assert!(matches!(
        bounded(endpoint.method_rx.recv()).await,
        Some(Method::ListWorkers)
    ));
    assert_eq!(
        bounded(host).await,
        Err(WorkerCommandSendError::NotDelivered {
            command_id: None,
            command: WorkerCommandKind::Cancel,
            reason: WorkerCommandNotDeliveredReason::CommandIdExhausted,
        })
    );
    assert_eq!(handle.shared_state.snapshot().last_command_id, u64::MAX);
}

#[tokio::test(start_paused = true)]
async fn bounded_wait_timeout_does_not_reissue_and_original_request_can_still_receive_admission() {
    let temp = TempDir::new().unwrap();
    let _sandbox =
        crate::runtime::worker_allocation::test_util::RuntimeDirSandbox::new(temp.path());
    let (mut endpoint, _, _) = initialized(&temp).await;
    let handle = endpoint.handle.clone();
    let response = handle.send_host_command(WorkerCommandKind::Cancel);
    tokio::pin!(response);
    assert!(poll!(&mut response).is_pending());
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut response)
            .await
            .is_err()
    );
    assert_eq!(handle.shared_state.snapshot().last_command_id, 0);
    let method = bounded(endpoint.method_rx.recv()).await.unwrap();
    let (envelope, kind) = commands::command_identity(&method).unwrap();
    assert_eq!(envelope.command_id, 1);
    assert!(
        endpoint.method_rx.is_empty(),
        "no automatic reissue on timeout"
    );
    reject_invalid_command_state(
        &handle.working_event_tx,
        &handle.shared_state,
        envelope,
        kind,
    );
    let acknowledgement = bounded(response).await.unwrap();
    assert_eq!(acknowledgement.command_id, 1);
    assert_eq!(
        acknowledgement.disposition,
        WorkerCommandDisposition::InvalidState
    );
}
