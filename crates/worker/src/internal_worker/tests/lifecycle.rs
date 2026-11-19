//! Real Worker/Engine/actor/ephemeral Store connections. Only the model is
//! scripted. These tests establish execution facts, not parent model processing
//! or persistence beyond the in-process Store lifetime; normal worker lib suite.
use super::*;
use InternalWorkerLifecycleFact as Fact;
use std::time::Duration;
use tokio::sync::mpsc;

async fn actor(
    client: Box<dyn LlmClient>,
    limit: Option<u32>,
) -> (
    InternalWorkerSessionHandle,
    mpsc::UnboundedReceiver<InternalWorkerLifecycleEvent>,
    mpsc::Sender<()>,
) {
    configured_actor(client, limit, |_| {}).await
}

async fn configured_actor(
    client: Box<dyn LlmClient>,
    limit: Option<u32>,
    configure: impl FnOnce(&mut Worker<Box<dyn LlmClient>, EphemeralSessionStore>),
) -> (
    InternalWorkerSessionHandle,
    mpsc::UnboundedReceiver<InternalWorkerLifecycleEvent>,
    mpsc::Sender<()>,
) {
    let store = EphemeralSessionStore::default();
    let mut engine =
        Engine::<_, agen::state::Mutable, crate::SessionHistoryMetadata>::new_annotated(client);
    engine.set_max_turns(limit);
    let cancel = engine.cancel_sender();
    let mut worker = Worker::new(
        manifest(),
        engine,
        store.clone(),
        WorkerWorkspaceContext::no_workspace(),
        WorkerFilesystemAuthority::None,
        Scope::empty(),
    )
    .await
    .unwrap();
    worker.disable_manifest_lifecycle_features();
    configure(&mut worker);
    let (tx, rx) = mpsc::unbounded_channel();
    let handle = prepare_internal_worker_session(
        worker,
        store,
        InternalWorkerVisibility::ParentClient,
        None,
        Some(Arc::new(move |event| {
            let _ = tx.send(event);
        })),
        None,
    )
    .await
    .unwrap();
    (handle, rx, cancel)
}

async fn next(
    rx: &mut mpsc::UnboundedReceiver<InternalWorkerLifecycleEvent>,
) -> InternalWorkerLifecycleEvent {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap()
}

fn finished() -> Box<dyn LlmClient> {
    Box::new(OneTurnClient {
        calls: Arc::default(),
    })
}

fn held() -> (Box<dyn LlmClient>, Arc<tokio::sync::Notify>) {
    let entered = Arc::new(tokio::sync::Notify::new());
    (
        Box::new(PendingClient {
            calls: Arc::default(),
            entered: entered.clone(),
        }),
        entered,
    )
}

#[tokio::test]
async fn continued_runs_and_concurrent_idle_stops_keep_separate_source_facts() {
    let (handle, mut events, _) = actor(finished(), None).await;
    let mut runs = Vec::new();
    for _ in 0..2 {
        handle.send("same input").await.unwrap();
        runs.push(next(&mut events).await);
        assert_eq!(
            handle.wait_until_idle().await,
            InternalWorkerSessionStatus::Idle
        );
    }
    let (left, right) = tokio::join!(handle.stop(), handle.stop());
    left.unwrap();
    right.unwrap();
    let stop = next(&mut events).await;
    handle.stop().await.unwrap();
    assert!(events.try_recv().is_err());
    assert_eq!(runs[0].fact, Fact::RunFinished);
    assert_eq!(runs[1].fact, Fact::RunFinished);
    assert_ne!(runs[0].source_id, runs[1].source_id);
    assert_ne!(runs[0].input_id, runs[1].input_id);
    assert_eq!(
        stop.fact,
        Fact::Stopped {
            reason: "stop_requested"
        }
    );
    assert_ne!(runs[1].source_id, stop.source_id);
    for event in runs.iter().chain([&stop]) {
        assert_eq!(event.session_id, handle.session_id_string());
        assert!(event.source_recorded);
        assert!(handle.entries().iter().any(|entry| matches!(entry,
            LogEntry::Extension { domain, payload, .. }
            if domain == "internal_worker_lifecycle" && payload["source_id"] == event.source_id)));
    }
    for run in runs {
        let id = run
            .terminal_entry_id
            .expect("only actual committed terminal references");
        assert!(handle.entries().iter().any(|entry| matches!(entry,
            LogEntry::RunCompleted { entry_id: Some(entry_id), .. } if entry_id.0 == id)));
    }
}

#[tokio::test]
async fn running_stop_and_cancel_preserve_the_run_result_before_separate_stop() {
    for manual_stop in [true, false] {
        let (client, entered) = held();
        let (handle, mut events, cancel) = actor(client, None).await;
        handle.send("hold").await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), entered.notified())
            .await
            .unwrap();
        if manual_stop {
            handle.stop().await.unwrap();
        } else {
            cancel.send(()).await.unwrap();
        }
        let run = next(&mut events).await;
        assert_eq!(run.fact, Fact::RunRolledBack);
        let terminal = run
            .terminal_entry_id
            .as_ref()
            .expect("rolled-back cancellation must reference its committed terminal");
        assert_eq!(
            handle
                .entries()
                .iter()
                .filter(|entry| matches!(entry,
                    LogEntry::RunCancelled { entry_id: Some(id), .. } if &id.0 == terminal
                ))
                .count(),
            1
        );
        assert!(
            !handle
                .entries()
                .iter()
                .any(|entry| matches!(entry, LogEntry::AnnotatedUserInput { .. }))
        );
        let stopped = next(&mut events).await;
        assert_eq!(
            stopped.fact,
            Fact::Stopped {
                reason: if manual_stop {
                    "stop_requested"
                } else {
                    "run_terminal"
                }
            }
        );
        assert_ne!(run.source_id, stopped.source_id);
        assert!(matches!(
            handle.wait_for_turn_outcome().await,
            Some(InternalWorkerTurnOutcome::Completed(
                WorkerRunResult::Cancelled | WorkerRunResult::RolledBack
            ))
        ));
        assert_eq!(handle.status(), InternalWorkerSessionStatus::Stopped);
        handle.stop().await.unwrap();
    }
}

#[tokio::test]
async fn rolled_back_cancel_terminal_save_failure_is_not_reported_as_saved_cancellation() {
    let (client, entered) = held();
    let (handle, mut events, cancel) = actor(client, None).await;
    handle
        .store
        .fail_next_append_matching(|entry| matches!(entry, LogEntry::RunCancelled { .. }));
    handle.send("cancel before model output").await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    cancel.send(()).await.unwrap();
    let run = next(&mut events).await;
    assert_eq!(run.fact, Fact::RunFailed);
    assert!(run.terminal_entry_id.is_none());
    assert!(matches!(next(&mut events).await.fact, Fact::Stopped { .. }));
    assert!(matches!(
        handle.wait_for_turn_outcome().await,
        Some(InternalWorkerTurnOutcome::Failed { .. })
    ));
    assert!(
        !handle.entries().iter().any(|entry| matches!(
            entry,
            LogEntry::AnnotatedUserInput { .. } | LogEntry::RunCancelled { .. }
        )),
        "failed terminal save must not undo input rollback or fabricate committed cancellation"
    );
    handle.stop().await.unwrap();
}

#[derive(Clone)]
struct ReleasedClient {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Semaphore>,
}

#[async_trait]
impl LlmClient for ReleasedClient {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        request: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
    {
        self.entered.notify_one();
        self.release.acquire().await.unwrap().forget();
        finished().stream(request).await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completion_competing_with_stop_emits_one_run_fact_and_one_stop_fact() {
    let client = ReleasedClient {
        entered: Arc::default(),
        release: Arc::new(tokio::sync::Semaphore::new(0)),
    };
    let (handle, mut events, _) = actor(Box::new(client.clone()), None).await;
    handle.send("finish or stop").await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), client.entered.notified())
        .await
        .unwrap();
    client.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(5), async {
        let (first, second) = tokio::join!(handle.stop(), handle.stop());
        first.unwrap();
        second.unwrap();
    })
    .await
    .unwrap();
    let run = next(&mut events).await;
    let stop = next(&mut events).await;
    assert!(matches!(
        run.fact,
        Fact::RunFinished | Fact::RunCancelled | Fact::RunRolledBack
    ));
    assert_eq!(
        stop.fact,
        Fact::Stopped {
            reason: "stop_requested"
        }
    );
    assert_ne!(run.source_id, stop.source_id);
    assert_eq!(handle.status(), InternalWorkerSessionStatus::Stopped);
    assert!(events.recv().await.is_none());
    assert_eq!(
        handle
            .entries()
            .iter()
            .filter(|entry| matches!(entry,
                LogEntry::Extension { domain, .. } if domain == "internal_worker_lifecycle"
            ))
            .count(),
        2,
        "one saved source per fact despite competing stops"
    );
}

#[tokio::test]
async fn command_channel_closure_reports_idle_and_running_actor_shutdown() {
    for running in [false, true] {
        let (client, entered) = held();
        let (handle, mut events, _) = actor(client, None).await;
        if running {
            handle.send("hold").await.unwrap();
            tokio::time::timeout(Duration::from_secs(5), entered.notified())
                .await
                .unwrap();
        }
        drop(handle);
        if running {
            assert!(matches!(
                next(&mut events).await.fact,
                Fact::RunCancelled | Fact::RunRolledBack
            ));
        }
        assert_eq!(
            next(&mut events).await.fact,
            Fact::Stopped {
                reason: "command_channel_closed"
            }
        );
        assert!(events.recv().await.is_none());
    }
}

#[tokio::test]
async fn limit_and_provider_failure_are_not_reported_as_success() {
    for (client, limit, expected) in [
        (finished(), Some(0), Fact::RunLimitReached),
        (
            Box::new(FailingClient) as Box<dyn LlmClient>,
            None,
            Fact::RunInterrupted,
        ),
    ] {
        let (handle, mut events, _) = actor(client, limit).await;
        handle.send("run").await.unwrap();
        let event = next(&mut events).await;
        assert_eq!(event.fact, expected);
        assert!(event.terminal_entry_id.is_some());
        assert_eq!(
            next(&mut events).await.fact,
            Fact::Stopped {
                reason: "run_terminal"
            }
        );
        assert_eq!(
            handle.wait_until_idle().await,
            InternalWorkerSessionStatus::Stopped
        );
    }
}

#[derive(Clone)]
struct PanickingClient;
#[async_trait]
impl LlmClient for PanickingClient {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        _: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
    {
        panic!("private-provider-payload");
    }
}

#[tokio::test]
async fn actor_panic_notifies_unknown_without_fabricating_terminal_or_stop() {
    let (handle, mut events, _) = actor(Box::new(PanickingClient), None).await;
    handle.send("panic").await.unwrap();
    let event = next(&mut events).await;
    assert_eq!(event.fact, Fact::ExecutionUnknown);
    assert!(event.input_id.is_some());
    assert!(event.terminal_entry_id.is_none());
    assert!(!format!("{event:?}").contains("private-provider-payload"));
    assert_eq!(
        handle.wait_until_idle().await,
        InternalWorkerSessionStatus::Failed
    );
    assert!(handle.wait_for_turn_outcome().await.is_none());
    assert!(handle.stop().await.is_err());
    assert!(events.recv().await.is_none());
}

#[tokio::test]
async fn failed_terminal_and_source_appends_do_not_claim_saved_success() {
    let (handle, mut events, _) = actor(finished(), None).await;
    handle.send("successful prior run").await.unwrap();
    let prior = next(&mut events).await;
    assert!(prior.terminal_entry_id.is_some());
    assert_eq!(
        handle.wait_until_idle().await,
        InternalWorkerSessionStatus::Idle
    );
    handle
        .store
        .fail_next_append_matching(|entry| matches!(entry, LogEntry::RunCompleted { .. }));
    handle.send("fail terminal save").await.unwrap();
    let event = next(&mut events).await;
    assert_eq!(event.fact, Fact::RunFailed);
    assert_ne!(event.source_id, prior.source_id);
    assert!(
        event.terminal_entry_id.is_none(),
        "failed continuation must not borrow the prior run's terminal"
    );
    assert!(event.source_recorded);
    assert!(matches!(next(&mut events).await.fact, Fact::Stopped { .. }));
    handle.stop().await.unwrap();

    let (handle, mut events, _) = actor(finished(), None).await;
    handle.store.fail_next_append_matching(|entry| matches!(entry, LogEntry::Extension { domain, .. } if domain == "internal_worker_lifecycle"));
    handle.send("fail source save").await.unwrap();
    let event = next(&mut events).await;
    assert_eq!(event.fact, Fact::RunFinished);
    assert!(event.terminal_entry_id.is_some());
    assert!(!event.source_recorded);
    assert!(!handle.entries().iter().any(|entry| matches!(entry, LogEntry::Extension { domain, .. } if domain == "internal_worker_lifecycle")));
    handle.stop().await.unwrap();
}

#[derive(Clone)]
struct OutputThenHold(Arc<tokio::sync::Notify>);

#[async_trait]
impl LlmClient for OutputThenHold {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        _: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
    {
        use futures_util::StreamExt;
        let entered = self.0.clone();
        Ok(Box::pin(
            futures::stream::iter([
                Ok(LlmEvent::text_block_start(0)),
                Ok(LlmEvent::text_delta(0, "partial output")),
                Ok(LlmEvent::text_block_stop(0, None)),
            ])
            .chain(futures::stream::poll_fn(move |_| {
                entered.notify_one();
                std::task::Poll::Pending
            })),
        ))
    }
}

#[tokio::test]
async fn paused_run_is_not_completed_and_stop_reports_its_unfinished_execution() {
    // A pause before any AI output is intentionally rolled back by Worker; hold
    // after output so this exercises a real resumable pause instead.
    let entered = Arc::new(tokio::sync::Notify::new());
    let client = Box::new(OutputThenHold(entered.clone()));
    let mut pause = None;
    let (handle, mut events, _) = configured_actor(client, None, |worker| {
        pause = Some(worker.engine_mut().pause_sender());
    })
    .await;
    handle.send("pause").await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    pause.unwrap().send(()).await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), handle.wait_until_idle())
            .await
            .unwrap(),
        InternalWorkerSessionStatus::Paused
    );
    assert!(
        events.try_recv().is_err(),
        "paused run is not a terminal source"
    );
    handle.stop().await.unwrap();
    assert_eq!(next(&mut events).await.fact, Fact::ExecutionUnknown);
    assert_eq!(
        next(&mut events).await.fact,
        Fact::Stopped {
            reason: "stop_requested"
        }
    );
}

#[derive(Clone)]
struct FailedCleanup;
#[async_trait]
impl crate::hook::Hook<crate::hook::WorkerStopping> for FailedCleanup {
    async fn call(
        &self,
        _: &crate::hook::WorkerStoppingContext,
    ) -> Result<(), crate::hook::HookError> {
        Err(crate::hook::HookError::new(
            crate::hook::HookErrorCategory::Dependency,
            "cleanup failed",
        ))
    }
}
impl crate::feature::FeatureModule for FailedCleanup {
    fn descriptor(&self) -> crate::feature::FeatureDescriptor {
        crate::feature::FeatureDescriptor::builtin("failed-cleanup", "Failed cleanup").with_hook(
            crate::feature::HookDeclaration::new(
                "cleanup",
                crate::feature::FeatureHookPoint::WorkerStopping,
            ),
        )
    }
    fn install(
        &self,
        context: &mut crate::feature::FeatureInstallContext<'_>,
    ) -> Result<(), crate::feature::FeatureInstallError> {
        context.hooks().add_worker_stopping(
            "cleanup",
            crate::hook::HookExecutionPolicy::fail_closed(),
            self.clone(),
        )?;
        Ok(())
    }
}

#[tokio::test]
async fn cleanup_failure_is_not_a_confirmed_stop_or_successful_stop_ack() {
    let (handle, mut events, _) = configured_actor(finished(), None, |worker| {
        let mut features = FeatureRegistryBuilder::new();
        features.add_module(FailedCleanup);
        let report = worker.install_features(features);
        assert!(report.reports.iter().all(|report| report.installed));
    })
    .await;
    handle.send("run").await.unwrap();
    assert_eq!(next(&mut events).await.fact, Fact::RunFinished);
    handle.wait_until_idle().await;
    assert!(handle.stop().await.is_err());
    assert_eq!(next(&mut events).await.fact, Fact::StopUnconfirmed);
    assert_eq!(handle.status(), InternalWorkerSessionStatus::Failed);
    assert!(handle.stop().await.is_err());
    assert!(events.recv().await.is_none());
}
