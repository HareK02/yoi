// Connection regression: real Internal Worker actors, ephemeral session stores,
// durable child-completion acceptance and the Engine interceptor. Only the model
// is scripted; no Runtime, external service or live Workspace is used.
const CHILD_COMPLETION: &str = "nested child completed durable work";

#[derive(Clone)]
struct NotificationStoreCheckingClient {
    scripted: ScriptedClient,
    store: crate::internal_worker::EphemeralSessionStore,
}

#[async_trait::async_trait]
impl LlmClient for NotificationStoreCheckingClient {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }

    async fn stream(
        &self,
        request: Request,
    ) -> Result<
        Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, ClientError>> + Send>>,
        ClientError,
    > {
        // Inspect the real session store at the model boundary, not after the
        // turn finishes: supplying an uncommitted notification must fail here.
        let mut committed = 0;
        for session in self.store.list_sessions().unwrap() {
            for segment in self.store.list_segments(session).unwrap() {
                committed += self.store.read_all(session, segment).unwrap().iter().filter(|entry| {
                    matches!(entry, session_store::LogEntry::AnnotatedSystemItem { entry, .. }
                        if matches!(&entry.item, session_store::SystemItem::Notification { message, .. }
                            if message == CHILD_COMPLETION))
                }).count();
            }
        }
        assert_eq!(
            format!("{:?}", request.items)
                .matches(CHILD_COMPLETION)
                .count(),
            committed
        );
        self.scripted.stream(request).await
    }
}

async fn notification_test_worker(
    client: ScriptedClient,
) -> (
    Worker<Box<dyn LlmClient>, crate::internal_worker::EphemeralSessionStore>,
    crate::internal_worker::EphemeralSessionStore,
) {
    let manifest = manifest::WorkerManifest::from_toml(
        "[worker]\nname = 'internal-notification-test'\n[model]\nscheme = 'anthropic'\nmodel_id = 'test'\n[engine]\n[scope]\n",
    )
    .unwrap();
    let store = crate::internal_worker::EphemeralSessionStore::default();
    let engine = Engine::<_, agen::state::Mutable, crate::SessionHistoryMetadata>::new_annotated(
        Box::new(NotificationStoreCheckingClient {
            scripted: client,
            store: store.clone(),
        }) as Box<dyn LlmClient>,
    );
    let mut worker = Worker::new(
        manifest,
        engine,
        store.clone(),
        crate::worker::WorkerWorkspaceContext::no_workspace(),
        crate::worker::WorkerFilesystemAuthority::None,
        manifest::Scope::empty(),
    )
    .await
    .unwrap();
    worker.disable_manifest_lifecycle_features();
    (worker, store)
}

#[tokio::test]
async fn internal_parent_commits_child_notification_once_before_model_supply() {
    use crate::internal_worker::{
        InternalWorkerSessionStatus, InternalWorkerVisibility, prepare_internal_worker_session,
    };
    use session_store::{LogEntry, SystemItem};

    let client = ScriptedClient::new(vec![
        finished_response(),
        finished_response(),
        finished_response(),
    ]);
    let requests = client.requests.clone();
    let (worker, store) = notification_test_worker(client).await;
    let pending = worker.pending_submission_handle();
    let target = durable_parent_notification_target(pending.clone(), worker.notify_buffer_handle());
    let crate::spawn::tool::ParentNotificationTarget::Durable(deliver) = target else {
        unreachable!();
    };
    let (parent_ended, mut parent_ends) = tokio::sync::mpsc::unbounded_channel();
    let parent = prepare_internal_worker_session(
        worker,
        store,
        InternalWorkerVisibility::ParentClient,
        None,
        Some(Arc::new(move |event| {
            let _ = parent_ended.send(event);
        })),
        None,
    )
    .await
    .unwrap();
    parent.send("first turn").await.unwrap();
    let initial = tokio::time::timeout(Duration::from_secs(10), parent.wait_until_idle())
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(10), parent_ends.recv())
        .await
        .unwrap()
        .unwrap();
    let message = CHILD_COMPLETION;
    let source = protocol::AuthenticatedInputSource::SubWorker {
        session_id: "nested-child".into(),
    };
    let notification = Method::NotifyTracked {
        receipt_scope: protocol::NotificationReceiptScope::Source,
        notification_request_id: "nested-completion-request".into(),
        message: message.into(),
        source: source.clone(),
    };
    let child_delivery = deliver.clone();
    let child_notification = notification.clone();
    let (child_worker, child_store) =
        notification_test_worker(ScriptedClient::new(vec![finished_response()])).await;
    let child = prepare_internal_worker_session(
        child_worker,
        child_store,
        InternalWorkerVisibility::ParentClient,
        None,
        Some(Arc::new(move |_| {
            // The same accepted delivery can be retried before the parent drains it.
            child_delivery(child_notification.clone()).unwrap();
            child_delivery(child_notification.clone()).unwrap();
        })),
        None,
    )
    .await
    .unwrap();
    child.send("finish nested work").await.unwrap();
    let child_status = tokio::time::timeout(Duration::from_secs(10), child.wait_until_idle())
        .await
        .unwrap();
    // stop waits for the actor callback even when its Idle flag was observed first.
    child.stop().await.unwrap();

    tokio::time::timeout(Duration::from_secs(10), parent_ends.recv())
        .await
        .unwrap()
        .unwrap();
    let notified = tokio::time::timeout(Duration::from_secs(10), parent.wait_until_idle())
        .await
        .unwrap();
    let mut replayed = None;
    if notified == InternalWorkerSessionStatus::Idle {
        deliver(notification).unwrap();
        parent.send("continue after receipt replay").await.unwrap();
        replayed = Some(
            tokio::time::timeout(Duration::from_secs(10), parent.wait_until_idle())
                .await
                .unwrap(),
        );
    }
    let snapshot = parent.protocol_snapshot();
    parent.stop().await.unwrap();

    assert_eq!(initial, InternalWorkerSessionStatus::Idle);
    assert_eq!(child_status, InternalWorkerSessionStatus::Idle);
    assert_eq!(notified, InternalWorkerSessionStatus::Idle, "{snapshot:?}");
    assert_eq!(replayed, Some(InternalWorkerSessionStatus::Idle));
    let entries = parent.entries();
    let notifications = entries.iter().filter_map(|entry| match entry {
        LogEntry::AnnotatedSystemItem { entry, .. }
            if matches!(&entry.item, SystemItem::Notification { message: text, .. } if text == message) => Some(entry),
        _ => None,
    }).collect::<Vec<_>>();
    assert_eq!(
        notifications.len(),
        1,
        "receipt replay must not duplicate history"
    );
    assert_eq!(
        notifications[0].metadata.origin,
        crate::worker::authenticated_input_provenance(&source)
    );
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert!(!format!("{:?}", requests[0].items).contains(message));
    for request in &requests[1..] {
        // Notification items contain the rendered body once, not the raw receipt.
        assert_eq!(format!("{:?}", request.items).matches(message).count(), 1);
    }
}

#[tokio::test]
async fn lifecycle_pending_save_failure_and_lost_acceptance_reply_retry_one_source() {
    use crate::internal_worker::{InternalWorkerLifecycleEvent, InternalWorkerLifecycleFact};
    use crate::spawn::tool::ParentNotificationTarget;
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("sessions");
    let backup = temp.path().join("held-sessions");
    let pending = crate::worker::PendingSubmissionHandle::for_test(&root);
    let buffer = NotifyBuffer::new();
    let ParentNotificationTarget::Durable(deliver) =
        durable_parent_notification_target(pending.clone(), buffer.clone())
    else {
        unreachable!()
    };
    // Actual FsStore append failure, followed by a lost reply after actual
    // acceptance. Neither the pending owner nor retry implementation is faked.
    std::fs::rename(&root, &backup).unwrap();
    std::fs::write(&root, b"not a directory").unwrap();
    let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls = attempts.clone();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let target = ParentNotificationTarget::Durable(Arc::new(move |method| {
        let attempt = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let result = deliver(method.clone());
        tx.send((method, result.is_ok())).unwrap();
        if attempt == 0 {
            assert!(result.is_err());
            std::fs::remove_file(&root).unwrap();
            std::fs::rename(&backup, &root).unwrap();
            return result;
        }
        result?;
        if attempt == 1 {
            return Err(std::io::Error::other("lost reply"));
        }
        Ok(())
    }));
    target.notify_lifecycle(
        "child",
        InternalWorkerLifecycleEvent {
            session_id: "ephemeral-child-session".into(),
            source_id: "stable-terminal-source".into(),
            input_id: Some("accepted-input".into()),
            timestamp_ms: 1,
            fact: InternalWorkerLifecycleFact::RunFinished,
            terminal_entry_id: Some("actual-terminal-entry".into()),
            source_recorded: true,
        },
    );
    let mut methods = Vec::new();
    for expected in [false, true, true] {
        let (method, accepted) = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(accepted, expected);
        methods.push(serde_json::to_value(method).unwrap());
    }
    assert_eq!(methods[0], methods[1]);
    assert_eq!(methods[1], methods[2]);
    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 3);
    assert_eq!(buffer.len(), 1);
    let records = pending.persisted_entries_for_test();
    assert_eq!(records.len(), 1, "one saved acceptance, not one per retry");
}

#[tokio::test(start_paused = true)]
async fn lifecycle_lost_reply_after_parent_commit_does_not_wake_or_commit_twice() {
    use crate::internal_worker::{
        InternalWorkerLifecycleEvent, InternalWorkerLifecycleFact, InternalWorkerSessionStatus,
        InternalWorkerVisibility, prepare_internal_worker_session,
    };
    use crate::spawn::tool::ParentNotificationTarget;
    let client = ScriptedClient::new(vec![finished_response(), finished_response()]);
    let requests = client.requests.clone();
    let (worker, store) = notification_test_worker(client).await;
    let buffer = worker.notify_buffer_handle();
    let pending = worker.pending_submission_handle();
    let ParentNotificationTarget::Durable(deliver) =
        durable_parent_notification_target(pending.clone(), buffer.clone())
    else {
        unreachable!()
    };
    let (ended, mut ends) = tokio::sync::mpsc::unbounded_channel();
    let parent = prepare_internal_worker_session(
        worker,
        store,
        InternalWorkerVisibility::ParentClient,
        None,
        Some(Arc::new(move |event| {
            let _ = ended.send(event);
        })),
        None,
    )
    .await
    .unwrap();
    let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls = attempts.clone();
    let (sent, mut deliveries) = tokio::sync::mpsc::unbounded_channel();
    let target = ParentNotificationTarget::Durable(Arc::new(move |method| {
        deliver(method.clone())?;
        sent.send(serde_json::to_value(method).unwrap()).unwrap();
        if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            return Err(std::io::Error::other("acceptance reply lost"));
        }
        Ok(())
    }));
    target.notify_lifecycle(
        "child",
        InternalWorkerLifecycleEvent {
            session_id: "child-session".into(),
            source_id: "committed-source".into(),
            input_id: Some("input".into()),
            timestamp_ms: 1,
            fact: InternalWorkerLifecycleFact::RunFinished,
            terminal_entry_id: None,
            source_recorded: false,
        },
    );
    let first = tokio::time::timeout(Duration::from_secs(5), deliveries.recv())
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), ends.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        parent.wait_until_idle().await,
        InternalWorkerSessionStatus::Idle
    );
    assert_eq!(pending.snapshot().notification_count, 0);
    assert!(buffer.is_empty());
    let method: Method = serde_json::from_value(first.clone()).unwrap();
    let Method::NotifyTracked { message, .. } = &method else {
        panic!("lifecycle delivery must retain tracked identity");
    };
    assert_eq!(parent.entries().iter().filter(|entry| matches!(entry,
        session_store::LogEntry::AnnotatedSystemItem { entry, .. }
        if matches!(&entry.item, session_store::SystemItem::Notification { message: body, .. } if body == message)
    )).count(), 1, "parent committed before retry");

    tokio::time::advance(Duration::from_millis(100)).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), deliveries.recv())
            .await
            .unwrap()
            .unwrap(),
        first
    );
    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert!(
        buffer.is_empty(),
        "accepted replay must not stage another notification"
    );
    parent
        .send("continue after committed replay")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), ends.recv())
        .await
        .unwrap()
        .unwrap();
    parent.stop().await.unwrap();
    assert_eq!(
        requests.lock().unwrap().len(),
        2,
        "retry must not start another model run"
    );
    for request in requests.lock().unwrap().iter() {
        assert_eq!(
            format!("{:?}", request.items)
                .matches("source=committed-source,")
                .count(),
            1
        );
    }
    assert_eq!(parent.entries().iter().filter(|entry| matches!(entry,
        session_store::LogEntry::AnnotatedSystemItem { entry, .. }
        if matches!(&entry.item, session_store::SystemItem::Notification { message: body, .. } if body == message)
    )).count(), 1);
}

#[tokio::test(start_paused = true)]
async fn lifecycle_unavailable_parent_exhausts_bounded_retries_with_exact_identity() {
    use crate::internal_worker::{InternalWorkerLifecycleEvent, InternalWorkerLifecycleFact};
    use crate::spawn::tool::ParentNotificationTarget;
    let (sent, mut deliveries) = tokio::sync::mpsc::unbounded_channel();
    let target = ParentNotificationTarget::Durable(Arc::new(move |method| {
        sent.send(serde_json::to_value(method).unwrap()).unwrap();
        Err(std::io::Error::other("parent pending store unavailable"))
    }));
    target.notify_lifecycle(
        "child",
        InternalWorkerLifecycleEvent {
            session_id: "child-session".into(),
            source_id: "unavailable-source".into(),
            input_id: None,
            timestamp_ms: 1,
            fact: InternalWorkerLifecycleFact::ExecutionUnknown,
            terminal_entry_id: None,
            source_recorded: false,
        },
    );
    drop(target);
    let first = tokio::time::timeout(Duration::from_secs(5), deliveries.recv())
        .await
        .unwrap()
        .unwrap();
    for _ in 0..3 {
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), deliveries.recv())
                .await
                .unwrap()
                .unwrap(),
            first
        );
    }
    // Channel closure proves the retry task released its target after exhaustion,
    // rather than guessing completion with a sleep or waiting for another attempt.
    assert!(
        tokio::time::timeout(Duration::from_secs(5), deliveries.recv())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn internal_notification_commit_failure_keeps_pending_input_out_of_model_supply() {
    use crate::internal_worker::{
        InternalWorkerLifecycleFact, InternalWorkerVisibility, prepare_internal_worker_session,
    };
    let client = ScriptedClient::new(vec![finished_response()]);
    let requests = client.requests.clone();
    let (worker, store) = notification_test_worker(client).await;
    let buffer = worker.notify_buffer_handle();
    let target =
        durable_parent_notification_target(worker.pending_submission_handle(), buffer.clone());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let parent = prepare_internal_worker_session(
        worker,
        store.clone(),
        InternalWorkerVisibility::ParentClient,
        None,
        Some(Arc::new(move |event| {
            let _ = tx.send(event);
        })),
        None,
    )
    .await
    .unwrap();
    parent.send("initial").await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    parent.wait_until_idle().await;
    store.fail_next_append_matching(|entry| {
        matches!(entry, session_store::LogEntry::AnnotatedSystemItem { entry, .. }
        if matches!(&entry.item, session_store::SystemItem::Notification { .. }))
    });
    target
        .notify(
            "child-session".into(),
            "commit-failure-source".into(),
            CHILD_COMPLETION.into(),
        )
        .unwrap();
    let failed = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        failed.fact,
        InternalWorkerLifecycleFact::RunInterrupted | InternalWorkerLifecycleFact::RunFailed
    ));
    parent.wait_until_idle().await;
    assert!(
        buffer.has_notification_pending(),
        "failed commit must requeue the tracked notification"
    );
    assert_eq!(
        requests.lock().unwrap().len(),
        1,
        "uncommitted notification must never reach a model request"
    );
    assert!(!parent.entries().iter().any(
        |entry| matches!(entry, session_store::LogEntry::AnnotatedSystemItem { entry, .. }
        if matches!(&entry.item, session_store::SystemItem::Notification { .. }))
    ));
    parent.stop().await.unwrap();
}
