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
    let parent = prepare_internal_worker_session(
        worker,
        store,
        InternalWorkerVisibility::ParentClient,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    parent.send("first turn").await.unwrap();
    let initial = tokio::time::timeout(Duration::from_secs(10), parent.wait_until_idle())
        .await
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
            child_delivery(child_notification.clone());
            child_delivery(child_notification.clone());
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

    parent.send("consume child completion").await.unwrap();
    let notified = tokio::time::timeout(Duration::from_secs(10), parent.wait_until_idle())
        .await
        .unwrap();
    let mut replayed = None;
    if notified == InternalWorkerSessionStatus::Idle {
        deliver(notification);
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
