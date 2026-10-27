//! Production Direct bootstrap, Controller admission, pending persistence and FsStore.
//! Only the provider is replaced by an explicitly released async boundary; no live Host,
//! Journal policy, socket or model continuation semantics are claimed.
use super::*;
use crate::{
    PreparedWorker, WorkerBootstrap, WorkerBootstrapLayout, WorkerFilesystemAuthority,
    WorkerManifest, WorkerWorkspaceContext,
};
use session_store::{CombinedStore, FsStore, FsWorkerStore};
use tempfile::TempDir;

#[derive(Clone)]
struct HeldProvider {
    started: mpsc::UnboundedSender<()>,
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
        self.started.send(()).unwrap();
        self.finish.notified().await;
        Ok(Box::pin(futures_util::stream::iter([Ok(
            agen::llm_client::event::Event::Status(agen::llm_client::event::StatusEvent {
                status: agen::llm_client::event::ResponseStatus::Completed,
            }),
        )])))
    }
}
async fn prepared(
    temp: &TempDir,
    client: HeldProvider,
) -> PreparedWorker<Box<dyn LlmClient>, CombinedStore<FsStore, FsWorkerStore>> {
    let mut manifest = WorkerManifest::from_toml(&format!("[worker]\nname='release-{}'\n[model]\nscheme='anthropic'\nmodel_id='scripted'\n[engine]\n[scope]\nallow=[]", uuid::Uuid::now_v7())).unwrap();
    manifest.compaction = Some(manifest::CompactionConfig {
        retained_tokens: 0,
        ..Default::default()
    });
    WorkerBootstrap::new(
        manifest,
        CombinedStore::new(
            FsStore::new(temp.path().join("sessions")).unwrap(),
            FsWorkerStore::new(temp.path().join("workers")).unwrap(),
        ),
        crate::PromptCatalogSource::builtins_only(),
        WorkerWorkspaceContext::no_workspace(),
        WorkerFilesystemAuthority::None,
        WorkerBootstrapLayout::Direct {
            runtime_base: temp.path().join("runtime"),
        },
    )
    .with_model_client(client)
    .prepare()
    .await
    .unwrap()
}
async fn reserve(handle: &WorkerHandle) -> Result<crate::SubjectReleaseReservation, String> {
    tokio::time::timeout(Duration::from_secs(5), handle.reserve_subject_release())
        .await
        .unwrap()
}
async fn shutdown(controller: crate::BootstrappedWorker) {
    controller
        .handle
        .send(Method::Shutdown {
            command: WorkerCommandEnvelope::new(1),
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), controller.shutdown)
        .await
        .unwrap()
        .unwrap();
    controller.controller_task.await.unwrap();
}
fn extension() -> LogEntry {
    LogEntry::Extension {
        ts: 1,
        domain: "release-test".into(),
        payload: serde_json::json!({}),
    }
}

#[tokio::test]
async fn reservation_fences_ingress_fallback_and_history_drop_recovers_commit_stays_fenced() {
    let temp = TempDir::new().unwrap();
    let _sandbox =
        crate::runtime::worker_allocation::test_util::RuntimeDirSandbox::new(temp.path());
    let (started, mut starts) = mpsc::unbounded_channel();
    let prepared = prepared(
        &temp,
        HeldProvider {
            started,
            finish: Arc::default(),
        },
    )
    .await;
    let writer = prepared.worker().log_writer_handle();
    let pending = prepared.worker().pending_submission_handle();
    let buffer = prepared.worker().notify_buffer_handle();
    let capture = prepared.worker().committed_session_capture_handle();
    let extensions = prepared.worker().session_extension_handle();
    let fallback = durable_parent_notification_target(pending.clone(), buffer.clone());
    let controller = prepared.start().await.unwrap();
    let before = controller.handle.committed_entries().len();
    let expected = capture.capture().unwrap().location();
    let guard = reserve(&controller.handle).await.unwrap();
    assert!(
        controller
            .handle
            .send(Method::submit_text(
                protocol::new_submission_request_id(),
                "blocked"
            ))
            .await
            .is_err()
    );
    assert!(
        controller
            .handle
            .send(Method::Notify {
                notification_request_id: protocol::new_submission_request_id(),
                message: "blocked".into()
            })
            .await
            .is_err()
    );
    assert!(
        pending
            .accept_notification("blocked".into(), "blocked".into())
            .is_err()
    );
    fallback.notify("child-session".into(), "must not be admitted".into());
    assert!(buffer.is_empty());
    assert!(!pending.has_unfinished_admission());
    assert!(writer.append_entry(extension()).is_err());
    assert!(
        extensions
            .append_if_current(&expected, "release-test", serde_json::json!({}))
            .is_err()
    );
    assert_eq!(controller.handle.committed_entries().len(), before);
    assert!(starts.try_recv().is_err());
    drop(guard);
    writer.append_entry(extension()).unwrap();
    let mut guard = reserve(&controller.handle).await.unwrap();
    guard.commit_disconnect().unwrap();
    guard.commit_disconnect().unwrap();
    drop(guard);
    assert!(writer.append_entry(extension()).is_err());
    assert!(
        pending
            .accept_notification("after-commit".into(), "blocked".into())
            .is_err()
    );
    assert!(
        controller
            .handle
            .send(Method::Notify {
                notification_request_id: protocol::new_submission_request_id(),
                message: "blocked".into()
            })
            .await
            .is_err()
    );
    assert!(reserve(&controller.handle).await.is_err());
    let _ = controller.handle.snapshot_event();
    controller
        .handle
        .send(Method::ListPendingSubmissions)
        .await
        .unwrap();
    let handle = controller.handle.clone();
    shutdown(controller).await;
    assert!(handle.protocol_is_closed());
    assert!(reserve(&handle).await.is_err());
}

#[tokio::test]
async fn accepted_input_cannot_be_overtaken_by_release_and_running_request_is_rejected() {
    let temp = TempDir::new().unwrap();
    let _sandbox =
        crate::runtime::worker_allocation::test_util::RuntimeDirSandbox::new(temp.path());
    let (started, mut starts) = mpsc::unbounded_channel();
    let finish = Arc::new(tokio::sync::Notify::new());
    let controller = prepared(
        &temp,
        HeldProvider {
            started,
            finish: finish.clone(),
        },
    )
    .await
    .start()
    .await
    .unwrap();
    let mut events = controller.handle.subscribe();
    // Enqueue succeeds before submitting the separate internal release request.
    // Either the queue or the held provider is observed; neither is quiescence.
    controller
        .handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "accepted first",
        ))
        .await
        .unwrap();
    assert!(reserve(&controller.handle).await.is_err());
    tokio::time::timeout(Duration::from_secs(5), starts.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(reserve(&controller.handle).await.is_err());
    finish.notify_one();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop { if matches!(events.recv().await.unwrap(), Event::WorkerState { snapshot } if snapshot.state == WorkerState::Idle) { break; } }
    }).await.unwrap();
    let guard = reserve(&controller.handle).await.unwrap();
    drop(guard);
    shutdown(controller).await;
}

#[tokio::test]
async fn staged_notification_fallback_blocks_release_until_the_actual_turn_finishes() {
    let temp = TempDir::new().unwrap();
    let _sandbox =
        crate::runtime::worker_allocation::test_util::RuntimeDirSandbox::new(temp.path());
    let (started, mut starts) = mpsc::unbounded_channel();
    let finish = Arc::new(tokio::sync::Notify::new());
    let prepared = prepared(
        &temp,
        HeldProvider {
            started,
            finish: finish.clone(),
        },
    )
    .await;
    let pending = prepared.worker().pending_submission_handle();
    let buffer = prepared.worker().notify_buffer_handle();
    let fallback = durable_parent_notification_target(pending.clone(), buffer);
    let controller = prepared.start().await.unwrap();
    let mut events = controller.handle.subscribe();
    fallback.notify("child".into(), "durable accepted notification".into());
    assert!(reserve(&controller.handle).await.is_err());
    tokio::time::timeout(Duration::from_secs(5), starts.recv())
        .await
        .unwrap()
        .unwrap();
    finish.notify_one();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop { if matches!(events.recv().await.unwrap(), Event::WorkerState { snapshot } if snapshot.state == WorkerState::Idle) { break; } }
    }).await.unwrap();
    let guard = reserve(&controller.handle).await.unwrap();
    drop(guard);
    shutdown(controller).await;
}

#[derive(Clone)]
struct HeldRewrite {
    started: mpsc::UnboundedSender<()>,
    finish: Arc<tokio::sync::Notify>,
}
impl crate::feature::FeatureModule for HeldRewrite {
    fn descriptor(&self) -> crate::feature::FeatureDescriptor {
        crate::feature::FeatureDescriptor::builtin("held-rewrite", "Held rewrite").with_hook(
            crate::feature::HookDeclaration::new(
                "hold",
                crate::feature::FeatureHookPoint::BeforeSessionRewrite,
            ),
        )
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
                "stop before compactor boundary".into(),
            ));
        }
        Ok(crate::hook::BeforeSessionRewriteAction::Continue)
    }
}

#[tokio::test]
async fn compact_maintenance_rejects_release_until_the_real_rewrite_boundary_exits() {
    let temp = TempDir::new().unwrap();
    let _sandbox =
        crate::runtime::worker_allocation::test_util::RuntimeDirSandbox::new(temp.path());
    let (started, mut starts) = mpsc::unbounded_channel();
    let finish = Arc::new(tokio::sync::Notify::new());
    let mut prepared = prepared(
        &temp,
        HeldProvider {
            started,
            finish: finish.clone(),
        },
    )
    .await;
    let (rewrite_started, mut rewrites) = mpsc::unbounded_channel();
    let rewrite_finish = Arc::new(tokio::sync::Notify::new());
    let mut registry = FeatureRegistryBuilder::new();
    registry.add_module(HeldRewrite {
        started: rewrite_started,
        finish: rewrite_finish.clone(),
    });
    assert!(
        !prepared
            .worker_mut()
            .install_features(registry)
            .has_errors()
    );
    let controller = prepared.start().await.unwrap();
    let mut events = controller.handle.subscribe();
    controller
        .handle
        .send(Method::submit_text(
            protocol::new_submission_request_id(),
            "history to compact ".repeat(128),
        ))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), starts.recv())
        .await
        .unwrap()
        .unwrap();
    finish.notify_one();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop { if matches!(events.recv().await.unwrap(), Event::WorkerState { snapshot } if snapshot.state == WorkerState::Idle) { break; } }
    }).await.unwrap();
    controller
        .handle
        .send(Method::Compact {
            command: WorkerCommandEnvelope::new(1),
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), rewrites.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(reserve(&controller.handle).await.is_err());
    assert!(matches!(
        controller.handle.shared_state.snapshot().state,
        WorkerState::Busy(WorkerBusyState::Maintenance(_))
    ));
    rewrite_finish.notify_one();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop { if matches!(events.recv().await.unwrap(), Event::WorkerState { snapshot } if snapshot.state == WorkerState::Idle) { break; } }
    }).await.unwrap();
    let guard = reserve(&controller.handle).await.unwrap();
    drop(guard);
    controller
        .handle
        .send(Method::Shutdown {
            command: WorkerCommandEnvelope::new(2),
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), controller.shutdown)
        .await
        .unwrap()
        .unwrap();
    controller.controller_task.await.unwrap();
}

#[tokio::test]
async fn queued_or_actor_staged_work_cannot_pass_the_exclusive_ack_boundary() {
    let temp = TempDir::new().unwrap();
    let _sandbox =
        crate::runtime::worker_allocation::test_util::RuntimeDirSandbox::new(temp.path());
    let (started, _) = mpsc::unbounded_channel();
    let mut prepared = prepared(
        &temp,
        HeldProvider {
            started,
            finish: Arc::default(),
        },
    )
    .await;
    prepared
        .worker_mut()
        .materialize_durable_session_head()
        .await
        .unwrap();
    let pending = prepared.worker().pending_submission_handle();
    let buffer = prepared.worker().notify_buffer_handle();
    let shared = WorkerSharedState::new(
        "queue-boundary".into(),
        prepared.worker().segment_id(),
        String::new(),
        build_greeting(prepared.worker()),
    );
    let control =
        crate::subject_release::ReleaseControl::new(prepared.worker().mutation_authority());
    for condition in [
        "queued",
        "staged",
        "attention",
        "closed",
        "durable-notification",
    ] {
        let (sender, receiver) = mpsc::channel(1);
        if condition == "queued" {
            sender
                .send(Method::submit_text(
                    protocol::new_submission_request_id(),
                    "already enqueued",
                ))
                .await
                .unwrap();
        }
        if condition == "durable-notification" {
            pending
                .accept_notification("already-admitted".into(), "pending".into())
                .unwrap();
        }
        if condition == "closed" {
            drop(sender);
        }
        let request = tokio::spawn({
            let control = control.clone();
            async move { control.request().await }
        });
        control.notified().await;
        answer_idle_release(
            &control,
            &shared,
            &receiver,
            condition == "staged",
            &pending,
            &buffer,
            condition == "attention",
        );
        assert!(request.await.unwrap().is_err(), "{condition}");
        // A failed request must not leave admission fenced for recovery.
        assert!(prepared.worker().mutation_authority().enter().is_ok());
    }
}
