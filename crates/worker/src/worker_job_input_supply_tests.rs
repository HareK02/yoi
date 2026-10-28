use super::*;
use agen::llm_client::event::{Event, ResponseStatus, StatusEvent};

type TestStore = session_store::CombinedStore<session_store::FsStore, session_store::FsWorkerStore>;
const SECRET: &str = "retained-frozen-subject-job-input";

#[derive(Debug)]
struct Host {
    operation: &'static str,
    status: AtomicUsize,
    checks: AtomicUsize,
}
impl Host {
    fn new(consolidation: bool) -> Self {
        Self {
            operation: if consolidation {
                "memory.consolidation.supply"
            } else {
                "journal.job.input.supply"
            },
            status: AtomicUsize::new(200),
            checks: AtomicUsize::new(0),
        }
    }
}
impl WorkspaceClient for Host {
    fn workspace_id(&self) -> Option<&str> {
        Some("workspace-test")
    }
    fn kind(&self) -> &str {
        "retained-job-host"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn execute(
        &self,
        request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        assert_eq!(
            request.path,
            "/api/w/workspace-test/authorization/bound-check"
        );
        let input: server_api::authorization::BoundOperationAuthorizationRequest =
            serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
        assert_eq!(input.operation, self.operation);
        assert_eq!(
            input.target,
            server_api::authorization::BoundAuthorizationTarget::Subject {}
        );
        self.checks.fetch_add(1, Ordering::SeqCst);
        Ok(WorkspaceResponse { status: self.status.load(Ordering::SeqCst) as u16,
            body: r#"{"scope":{"kind":"subject","workspace_id":"workspace-test","subject_id":"subject"}}"#.into() })
    }
}

#[derive(Clone, Default)]
struct Provider {
    requests: Arc<Mutex<Vec<agen::llm_client::Request>>>,
    compactor_calls: Arc<AtomicUsize>,
    revoke_on_compact: Option<Arc<Host>>,
    invalid_first_summary: bool,
}
#[async_trait]
impl LlmClient for Provider {
    async fn stream(
        &self,
        request: agen::llm_client::Request,
    ) -> Result<agen::llm_client::ResponseStream, agen::llm_client::ClientError> {
        let compact = request
            .tools
            .iter()
            .any(|tool| tool.name == "write_summary");
        self.requests.lock().unwrap().push(request);
        let events = if compact && self.compactor_calls.fetch_add(1, Ordering::SeqCst) % 2 == 0 {
            if let Some(host) = &self.revoke_on_compact {
                host.status.store(403, Ordering::SeqCst);
            }
            vec![
                Event::tool_use_start(0, "summary", "write_summary"),
                Event::tool_input_delta(
                    0,
                    if self.invalid_first_summary {
                        serde_json::json!({"text":5}).to_string()
                    } else {
                        serde_json::json!({"text":format!("Retained {SECRET}")}).to_string()
                    },
                ),
                Event::tool_use_stop(0),
            ]
        } else {
            vec![
                Event::text_block_start(0),
                Event::text_delta(0, "done"),
                Event::text_block_stop(0, None),
            ]
        };
        Ok(Box::pin(futures::stream::iter(
            events
                .into_iter()
                .chain(std::iter::once(Event::Status(StatusEvent {
                    status: ResponseStatus::Completed,
                })))
                .map(Ok),
        )))
    }
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
}

fn binding(consolidation: bool) -> crate::BackendJobExecutionBinding {
    crate::BackendJobExecutionBinding {
        job_id: "bound-job".into(),
        attempt_id: "bound-attempt".into(),
        input_digest: Some("digest".into()),
        subjektiv_consolidation: consolidation,
        subjektiv_journal: !consolidation,
    }
}
fn store(temp: &tempfile::TempDir) -> TestStore {
    session_store::CombinedStore::new(
        session_store::FsStore::new(temp.path().join("sessions")).unwrap(),
        session_store::FsWorkerStore::new(temp.path().join("workers")).unwrap(),
    )
}
fn manifest() -> WorkerManifest {
    let mut manifest = minimal_manifest();
    manifest.worker.name = format!("retained-job-{}", uuid::Uuid::now_v7());
    manifest.scope.allow.clear();
    manifest.compaction = Some(manifest::CompactionConfig {
        model: None,
        retained_tokens: 1,
        ..Default::default()
    });
    manifest
}
async fn boot(
    manifest: WorkerManifest,
    store: TestStore,
    host: Arc<Host>,
    provider: Provider,
    consolidation: bool,
) -> Worker<Provider, TestStore> {
    let mut worker = Worker::new(
        manifest,
        Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(provider),
        store,
        WorkerWorkspaceContext::with_client(None, host),
        WorkerFilesystemAuthority::None,
        Scope::empty(),
    )
    .await
    .unwrap();
    worker.bind_backend_job(binding(consolidation)).unwrap();
    worker
        .engine_mut()
        .set_system_prompt("Host-bound frozen Job input");
    worker
}
async fn restore(
    manifest: WorkerManifest,
    store: TestStore,
    session: SessionId,
    segment: SegmentId,
    host: Arc<Host>,
    provider: Provider,
) -> Worker<Box<dyn LlmClient>, TestStore> {
    Worker::<Box<dyn LlmClient>, _>::restore_from_manifest_with_context_and_model_client(
        session,
        segment,
        manifest,
        store,
        PromptCatalogSource::builtins_only(),
        WorkerWorkspaceContext::with_client(None, host),
        WorkerFilesystemAuthority::None,
        Some(Box::new(provider)),
    )
    .await
    .unwrap()
}
fn assert_retained(store: &TestStore, session: SessionId, segment: SegmentId, consolidation: bool) {
    let kind = ProtectedInputKind::for_job(&binding(consolidation))
        .unwrap()
        .as_str();
    assert!(store.read_all(session, segment).unwrap().iter().any(
        |entry| matches!(entry, LogEntry::Extension { domain, payload, .. }
            if domain == PROTECTED_INPUTS_DOMAIN && payload["kinds"] == serde_json::json!([kind]))
    ));
}

#[tokio::test]
async fn subject_job_requirement_survives_fork_compaction_rewind_and_missing_or_wrong_restore_binding()
 {
    let _environment = worker_allocation::test_util::ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for consolidation in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let manifest = manifest();
        let host = Arc::new(Host::new(consolidation));
        let provider = Provider::default();
        let requests = provider.requests.clone();
        let mut worker = boot(
            manifest.clone(),
            store.clone(),
            host.clone(),
            provider,
            consolidation,
        )
        .await;
        worker.run_text(SECRET.repeat(100)).await.unwrap();
        assert!(format!("{:?}", requests.lock().unwrap()[0]).contains(SECRET));
        let session = worker.session_id();
        let first = worker.segment_id();
        assert_retained(&store, session, first, consolidation);
        store
            .append(
                session,
                first,
                &LogEntry::Extension {
                    ts: 1,
                    domain: "test.concurrent-write".into(),
                    payload: serde_json::json!({}),
                },
            )
            .unwrap();
        worker.run_text("fork frozen Job input").await.unwrap();
        let fork = worker.segment_id();
        assert_ne!(fork, first);
        assert_retained(&store, session, fork, consolidation);
        let compact = worker.compact(0).await.unwrap();
        assert_ne!(compact, fork);
        assert_retained(&store, session, compact, consolidation);
        worker.run_text("keep this turn").await.unwrap();
        worker.run_text("rewind this turn").await.unwrap();
        let target = rewind_test_inputs(&worker)[0].0.clone();
        worker.rewind_to(target).await.unwrap();
        let rewind = worker.segment_id();
        assert_retained(&store, session, rewind, consolidation);
        drop(worker);
        for segment in [first, fork, compact, rewind] {
            for mode in [
                "missing",
                "wrong-kind",
                "wrong-job",
                "wrong-digest",
                "revoked",
                "allowed",
            ] {
                let provider = Provider::default();
                let calls = provider.requests.clone();
                let host = Arc::new(Host::new(consolidation));
                if mode == "revoked" {
                    host.status.store(403, Ordering::SeqCst);
                }
                let mut restored = restore(
                    manifest.clone(),
                    store.clone(),
                    session,
                    segment,
                    host.clone(),
                    provider,
                )
                .await;
                if mode != "missing" {
                    let mut bound = binding(if mode == "wrong-kind" {
                        !consolidation
                    } else {
                        consolidation
                    });
                    if mode == "wrong-job" {
                        bound.job_id = "other-job".into();
                    }
                    if mode == "wrong-digest" {
                        bound.input_digest = Some("other-input".into());
                    }
                    restored.bind_backend_job(bound).unwrap();
                }
                let outcome = restored
                    .run_text("restored frozen Job input")
                    .await
                    .unwrap();
                assert_eq!(
                    matches!(outcome, WorkerRunResult::Finished),
                    mode == "allowed",
                    "{mode}: {outcome:?}"
                );
                assert_eq!(
                    calls.lock().unwrap().len(),
                    usize::from(mode == "allowed"),
                    "{mode}"
                );
                if mode == "allowed" || mode == "revoked" {
                    assert!(host.checks.load(Ordering::SeqCst) > 0);
                }
            }
        }
    }
}

#[test]
fn malformed_subject_job_provenance_never_becomes_a_local_or_new_job_grant() {
    for payload in [
        serde_json::json!({"version":1,"kinds":["subject-journal-job"]}),
        serde_json::json!({"version":1,"kinds":["subject-journal-job"],"subject_jobs":[{
            "kind":"subject-journal-job","job_id":"","input_digest":"digest"}]}),
        serde_json::json!({"version":1,"kinds":["subject-journal-job"],"subject_jobs":[{
            "kind":"workspace-memory","job_id":"job","input_digest":"digest"}]}),
    ] {
        let entry = LogEntry::Extension {
            ts: 1,
            domain: PROTECTED_INPUTS_DOMAIN.into(),
            payload,
        };
        assert!(ProtectedInputProvenance::restore(&[entry], &[], None).is_err());
    }
}

#[tokio::test]
async fn subject_job_compactor_reauthorizes_after_each_request_before_correction_or_retry() {
    let _environment = worker_allocation::test_util::ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for (consolidation, invalid_summary) in
        [(false, false), (false, true), (true, false), (true, true)]
    {
        let temp = tempfile::tempdir().unwrap();
        let host = Arc::new(Host::new(consolidation));
        let provider = Provider {
            revoke_on_compact: Some(host.clone()),
            invalid_first_summary: invalid_summary,
            ..Default::default()
        };
        let requests = provider.requests.clone();
        let compacts = provider.compactor_calls.clone();
        let mut worker = boot(manifest(), store(&temp), host, provider, consolidation).await;
        worker.run_text(SECRET.repeat(100)).await.unwrap();
        let segment = worker.segment_id();
        assert!(worker.compact(0).await.is_err());
        assert_eq!(
            worker.segment_id(),
            segment,
            "failed compaction cannot publish a summary"
        );
        assert_eq!(
            compacts.load(Ordering::SeqCst),
            1,
            "revocation stops every later compactor dispatch"
        );
        let requests = requests.lock().unwrap();
        assert_eq!(
            requests.len(),
            2,
            "one allowed foreground and one pre-revocation compactor request only"
        );
        assert!(
            format!("{:?}", requests[1]).contains(SECRET),
            "the allowed compactor actually received frozen input"
        );
    }
}
