//! Actual model-issued SubWorkerSpawn, scoped broker, Internal actors, notification
//! acceptance, Store commits and model supply. Only LLM transport is scripted;
//! isolated Workdirs and ephemeral sessions, selected by normal worker lib tests.
use super::*;
use crate::feature::builtin::worker_observation::SpawnedSubWorkerObservationProvider;
use crate::feature::contracts::{
    WorkerObservationError, WorkerObservationProvider, WorkerObservationSubjectRef,
};
use session_store::{LogEntry, Store, SystemItem};
use tokio::sync::{Notify, Semaphore, mpsc};

#[derive(Debug)]
struct RuntimeListClient {
    limit: usize,
}
impl WorkspaceClient for RuntimeListClient {
    fn workspace_id(&self) -> Option<&str> {
        Some("workspace-test")
    }
    fn kind(&self) -> &str {
        "runtime-list-fixture"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn authorize_operation_scope(
        &self,
        _: &str,
        _: &server_api::authorization::PermissionResource,
    ) -> Result<Option<server_api::authorization::PermissionResource>, WorkspaceClientError> {
        Ok(None)
    }
    fn execute(
        &self,
        request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        assert!(request.path.ends_with("/worker-control/workers"));
        Ok(WorkspaceResponse { status: 200, content_type: None, body: json!({
            "limit":self.limit, "truncated":false, "items":[{
                "subject":{"runtime_id":"r", "worker_id":"parent"}, "worker":"W-1",
                "display_name":"parent", "state":"idle", "relation":"peer", "permissions":["observe"]
            }]
        }).to_string() })
    }
}
async fn mixed_list(registry: Arc<SpawnedWorkerRegistry>, limit: usize) -> Value {
    let mut pending = Vec::new();
    let mut hooks = crate::HookRegistryBuilder::default();
    let report = crate::feature::FeatureRegistryBuilder::new()
        .with_module(
            crate::feature::builtin::manage_worker::manage_worker_feature(
                Arc::new(RuntimeListClient { limit }),
                Some(registry),
                false,
            ),
        )
        .install_into_pending(&mut pending, &mut hooks);
    assert!(report.reports[0].installed);
    let (_, tool) = pending
        .into_iter()
        .map(|definition| definition())
        .find(|(meta, _)| meta.name == "WorkerList")
        .unwrap();
    let output = tool
        .execute("{}", agen::tool::ToolExecutionContext::direct())
        .await
        .unwrap();
    serde_json::from_str(output.content.as_deref().unwrap()).unwrap()
}

#[derive(Clone)]
struct HeldChild(Arc<Semaphore>);
#[async_trait]
impl LlmClient for HeldChild {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        _: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
    {
        self.0.acquire().await.unwrap().forget();
        Ok(response(None))
    }
}

fn response(
    tool: Option<(&str, &str)>,
) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>> {
    let mut events = if let Some((name, input)) = tool {
        vec![
            LlmEvent::tool_use_start(0, uuid::Uuid::now_v7().to_string(), name),
            LlmEvent::tool_input_delta(0, input),
            LlmEvent::tool_use_stop(0),
        ]
    } else {
        vec![
            LlmEvent::text_block_start(0),
            LlmEvent::text_delta(0, "finished"),
            LlmEvent::text_block_stop(0, None),
        ]
    };
    events.push(LlmEvent::Status(StatusEvent {
        status: ResponseStatus::Completed,
    }));
    Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
}

#[derive(Clone)]
struct ParentClient {
    busy: bool,
    calls: Arc<AtomicUsize>,
    entered_second: Arc<Notify>,
    release_second: Arc<Notify>,
    store: EphemeralSessionStore,
    observed: mpsc::UnboundedSender<usize>,
}

#[async_trait]
impl LlmClient for ParentClient {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        request: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
    {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let committed = self.store.list_sessions().unwrap().into_iter().flat_map(|session| {
            self.store.list_segments(session).unwrap().into_iter().flat_map(move |segment| self.store.read_all(session, segment).unwrap())
        }).filter(|entry| matches!(entry, LogEntry::AnnotatedSystemItem { entry, .. }
            if matches!(&entry.item, SystemItem::Notification { message, .. } if message.contains("SubWorker `leaf` lifecycle")))).count();
        let supplied = format!("{:?}", request.items)
            .matches("SubWorker `leaf` lifecycle")
            .count();
        assert_eq!(
            supplied, committed,
            "nested notifications must be committed before model supply"
        );
        self.observed.send(supplied).unwrap();
        if call == 0 {
            return Ok(response(Some((
                "SubWorkerSpawn",
                r#"{"name":"leaf","task":"finish leaf","scope":[{"target":".","permission":"read"}]}"#,
            ))));
        }
        if call == 1 {
            self.entered_second.notify_one();
            if self.busy {
                self.release_second.notified().await;
                return Ok(response(Some(("WorkerList", "{}"))));
            }
        }
        Ok(response(None))
    }
}

#[tokio::test]
async fn actual_spawn_delivers_to_busy_and_idle_internal_parent_before_model_supply() {
    for busy in [true, false] {
        tokio::time::timeout(Duration::from_secs(15), async {
            let fixture = EnvironmentFixture::new(&[("checkout", "")]);
            let store = EphemeralSessionStore::default();
            let child_gate = Arc::new(Semaphore::new(0));
            let (observed, mut observations) = mpsc::unbounded_channel();
            let parent_client = ParentClient {
                busy, calls: Arc::default(), entered_second: Arc::default(), release_second: Arc::default(),
                store: store.clone(), observed,
            };
            let context = crate::worker::WorkerWorkspaceContext::local_filesystem(None)
                .with_internal_model_client(Box::new(HeldChild(child_gate.clone())));
            let mut worker = Worker::from_internal_manifest_with_context(
                fixture.manifest.clone(), store.clone(), PromptCatalogSource::builtins_only(), context,
                WorkerFilesystemAuthority::None, Some(Box::new(parent_client.clone()) as Box<dyn LlmClient>),
            ).await.unwrap();
            worker.bind_workdir_sessions(fixture.sessions.clone());
            register_worker_tools(&mut worker, fixture.runtime.path().into(), fixture.registry.clone(), None,
                Some(workdir::WorkdirToolBrokerRouter::new(fixture.sessions.clone()))).await.unwrap();
            let (ended, mut ends) = mpsc::unbounded_channel();
            let parent = prepare_internal_worker_session(worker, store, InternalWorkerVisibility::ParentClient,
                Some(fixture.registry.clone()), Some(Arc::new(move |event| { let _ = ended.send(event); })), None).await.unwrap();
            parent.send("spawn the leaf").await.unwrap();
            parent_client.entered_second.notified().await;
            assert_eq!(observations.recv().await.unwrap(), 0);
            assert_eq!(observations.recv().await.unwrap(), 0);
            let leaf = fixture.registry.get_internal("leaf").expect("real SubWorkerSpawn must register its child");
            if !busy {
                ends.recv().await.unwrap();
                assert_eq!(parent.wait_until_idle().await, InternalWorkerSessionStatus::Idle);
            }
            let mixed = mixed_list(fixture.registry.clone(), 10).await;
            assert_eq!(mixed["items"].as_array().unwrap().len(), 2);
            assert_eq!(mixed["items"][0]["state"], "idle", "child activity must not change the Runtime parent's input-state projection");
            assert_eq!(mixed["items"][1]["target"], json!({"kind":"sub_worker", "name":"leaf"}));
            assert_eq!(mixed["items"][1]["relation"], "direct_child");
            assert_eq!(mixed["items"][1]["state"], "running");
            assert!(mixed["items"][1].get("summary").is_none());
            let bounded = mixed_list(fixture.registry.clone(), 1).await;
            assert_eq!(bounded["items"].as_array().unwrap().len(), 1);
            assert_eq!(bounded["truncated"], true);
            child_gate.add_permits(1);
            assert_eq!(leaf.session.wait_until_idle().await, InternalWorkerSessionStatus::Idle);
            if busy { parent_client.release_second.notify_one(); }
            assert_eq!(observations.recv().await.unwrap(), 1);
            let completed = ends.recv().await.unwrap();
            assert_eq!(completed.fact, crate::internal_worker::InternalWorkerLifecycleFact::RunFinished);
            assert_eq!(parent.wait_until_idle().await, InternalWorkerSessionStatus::Idle);
            let entries = parent.entries();
            let notification = entries.iter().find_map(|entry| match entry {
                LogEntry::AnnotatedSystemItem { entry, .. } if matches!(&entry.item, SystemItem::Notification { message, .. } if message.contains("SubWorker `leaf` lifecycle")) => Some(entry),
                _ => None,
            }).unwrap();
            assert_eq!(notification.metadata.origin, crate::worker::authenticated_input_provenance(&protocol::AuthenticatedInputSource::SubWorker { session_id: leaf.session.session_id_string() }));
            // Continuation is a new fact, even for the same input and child.
            leaf.session.send("finish leaf").await.unwrap();
            child_gate.add_permits(1);
            assert_eq!(observations.recv().await.unwrap(), 2);
            ends.recv().await.unwrap();
            assert_eq!(parent.wait_until_idle().await, InternalWorkerSessionStatus::Idle);
            let observer = SpawnedSubWorkerObservationProvider::new(fixture.registry.clone());
            let subject = WorkerObservationSubjectRef::SubWorker { name: "leaf".into() };
            observer.capture_worker_session(&subject).await.expect("retained child remains observable after run completion");
            fixture.registry.remove_internal("leaf").await.unwrap().unwrap();
            assert_eq!(observations.recv().await.unwrap(), 3);
            ends.recv().await.unwrap();
            assert_eq!(parent.wait_until_idle().await, InternalWorkerSessionStatus::Idle);
            assert!(fixture.registry.get_internal("leaf").is_none());
            assert!(matches!(observer.capture_worker_session(&subject).await, Err(WorkerObservationError::NotFound)));
            assert!(observer.list_worker_sessions().await.unwrap().is_empty());
            let retained_notifications = parent.entries().iter().filter_map(|entry| match entry {
                LogEntry::AnnotatedSystemItem { entry, .. } => match &entry.item {
                    SystemItem::Notification { message, .. } if message.contains("SubWorker `leaf` lifecycle") => Some(message.clone()),
                    _ => None,
                },
                _ => None,
            }).collect::<Vec<_>>();
            assert_eq!(retained_notifications.len(), 3, "removal must not discard parent receipts or duplicate actor stop");
            assert!(retained_notifications.iter().all(|message| message.contains(&leaf.session.session_id_string()) && message.contains("removal/restart may make it unavailable")));
            parent.stop().await.unwrap();
        }).await.expect("actual nested spawn lifecycle must make progress without polling");
    }
}
