use super::*;
use std::sync::atomic::AtomicBool;
use worker::hook::{Hook, HookError, HookPreRequestAction, PreLlmRequest, PreRequestContext};

const FROZEN_INPUT: &str = "frozen-subject-job-input-must-not-leak";

// Real Controller/Worker + production supply guard; only the Host transport and
// model are replaced. Host policy/origin/attempt fences have signed Server tests.
#[derive(Debug)]
struct JobHost {
    operation: &'static str,
    denied: AtomicBool,
    checks: AtomicUsize,
}
impl JobHost {
    fn new(consolidation: bool) -> Self {
        Self {
            operation: if consolidation {
                "memory.consolidation.supply"
            } else {
                "journal.job.input.supply"
            },
            denied: AtomicBool::new(false),
            checks: AtomicUsize::new(0),
        }
    }
}
impl WorkspaceClient for JobHost {
    fn workspace_id(&self) -> Option<&str> {
        Some("workspace-test")
    }
    fn kind(&self) -> &str {
        "job-supply-host"
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
        Ok(WorkspaceResponse {
            status: if self.denied.load(Ordering::SeqCst) { 403 } else { 200 },
            body: r#"{"scope":{"kind":"subject","workspace_id":"workspace-test","subject_id":"bound-subject"}}"#.into(),
        })
    }
}

async fn job_worker(
    consolidation: bool,
    host: Arc<JobHost>,
) -> (Worker<MockClient, TestStore>, MockClient) {
    let workspace = tempfile::tempdir().unwrap();
    let profile = if consolidation {
        "subjektiv-memory-consolidation"
    } else {
        "subjektiv-journal-recording"
    };
    let mut manifest = ProfileResolver::new()
        .with_workspace_base(workspace.path())
        .resolve(
            &ProfileSelector::source_named(ProfileRegistrySource::Builtin, profile),
            ProfileResolveOptions::with_worker_name("subject-job-supply-test"),
        )
        .unwrap()
        .manifest;
    if consolidation {
        manifest
            .feature
            .subjektiv
            .bind_workspace_settings(manifest::WorkspaceMemorySettingsSnapshot {
                workspace_id: "workspace-test".into(),
                language: "English".into(),
            })
            .unwrap();
    }
    manifest.compaction = Some(manifest::CompactionConfig {
        retained_tokens: 1,
        model: None,
        ..Default::default()
    });
    let client = MockClient::new(simple_text_events());
    let (mut worker, _) = make_worker_with_manifest_and_workspace_context(
        client.clone(),
        manifest,
        WorkerWorkspaceContext::with_client(None, host),
    )
    .await;
    worker
        .bind_backend_job(worker::BackendJobExecutionBinding {
            job_id: "bound-job".into(),
            attempt_id: "bound-attempt".into(),
            input_digest: Some("bound-digest".into()),
            subjektiv_consolidation: consolidation,
            subjektiv_journal: !consolidation,
        })
        .unwrap();
    (worker, client)
}

#[tokio::test]
async fn idle_controller_compaction_rejects_revoked_subject_job_input_before_any_model_request() {
    for consolidation in [false, true] {
        let host = Arc::new(JobHost::new(consolidation));
        let (worker, client) = job_worker(consolidation, host.clone()).await;
        let handle = spawn_controller(worker).await;
        let mut events = handle.subscribe();
        let id = protocol::new_submission_request_id();
        handle
            .send(Method::submit_text(id.clone(), FROZEN_INPUT.repeat(200)))
            .await
            .unwrap();
        assert!(
            drain_until(
                &mut events,
                std::time::Duration::from_secs(2),
                |event| matches!(event,
            Event::WorkerState { snapshot } if snapshot.catalog_status() == WorkerStatus::Idle
                && snapshot.last_finished_submission_request_id.as_deref() == Some(id.as_str()))
            )
            .await
        );
        let before = client.captured_requests().len();
        assert_eq!(before, 1);
        assert!(format!("{:?}", client.captured_requests()[0]).contains(FROZEN_INPUT));
        let checks = host.checks.load(Ordering::SeqCst);
        assert!(checks > 0);
        host.denied.store(true, Ordering::SeqCst);
        handle
            .send(Method::Compact {
                command: worker_command(&handle),
            })
            .await
            .unwrap();
        assert!(drain_until(&mut events, std::time::Duration::from_secs(2), |event| matches!(event,
            Event::Error { message, .. } if message.contains(host.operation) && message.contains("403"))).await,
            "revoked Job compaction must report its operation denial");
        assert!(host.checks.load(Ordering::SeqCst) > checks);
        assert_eq!(
            client.captured_requests().len(),
            before,
            "revoked frozen input reached compactor"
        );
    }
}

struct AuthorizeThenRevoke(Arc<JobHost>);
#[async_trait]
impl Hook<PreLlmRequest> for AuthorizeThenRevoke {
    async fn call(&self, _: &PreRequestContext) -> Result<HookPreRequestAction, HookError> {
        self.0
            .authorize_subject_operation(self.0.operation)
            .unwrap();
        self.0.denied.store(true, Ordering::SeqCst);
        Ok(HookPreRequestAction::Continue)
    }
}

#[tokio::test]
async fn later_hook_revocation_rejects_subject_job_input_at_final_supply_boundary() {
    for consolidation in [false, true] {
        let host = Arc::new(JobHost::new(consolidation));
        let (mut worker, client) = job_worker(consolidation, host.clone()).await;
        worker.add_pre_llm_request_hook(AuthorizeThenRevoke(host.clone()));
        let outcome = worker.run_text(FROZEN_INPUT).await.unwrap();
        assert!(
            host.checks.load(Ordering::SeqCst) > 0,
            "the hook first observes allowed Host authority"
        );
        assert!(
            matches!(outcome, worker::WorkerRunResult::Interrupted { .. }),
            "{outcome:?}"
        );
        assert!(
            client.captured_requests().is_empty(),
            "same-request revocation leaked Job input"
        );
    }
}
