use super::*;
use agen::llm_client::{ClientError, LlmClient, Request as ModelRequest, ResponseStream};
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use worker::hook::{Hook, HookError, HookPreRequestAction, PreLlmRequest, PreRequestContext};

type ModelStore =
    session_store::CombinedStore<session_store::FsStore, session_store::FsWorkerStore>;

#[derive(Clone, Default)]
struct Model {
    calls: Arc<AtomicUsize>,
    input: Arc<Mutex<Vec<String>>>,
}
#[async_trait]
impl LlmClient for Model {
    async fn stream(
        &self,
        request: ModelRequest,
    ) -> std::result::Result<ResponseStream, ClientError> {
        use agen::llm_client::event::{Event, ResponseStatus, StatusEvent};
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.input.lock().unwrap().push(format!("{request:?}"));
        Ok(Box::pin(futures::stream::iter(
            [
                Event::text_block_start(0),
                Event::text_delta(0, "done"),
                Event::text_block_stop(0, None),
                Event::Status(StatusEvent {
                    status: ResponseStatus::Completed,
                }),
            ]
            .into_iter()
            .map(Ok),
        )))
    }
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
}

async fn job_fixture(
    consolidation: bool,
    missing_origin: bool,
) -> (
    tempfile::TempDir,
    WorkspaceApi,
    crate::backend_job::BackendJobReservation,
) {
    if !consolidation {
        return super::super::subjektiv_journal_tests::model_supply_job_fixture(missing_origin)
            .await;
    }
    let f = Fixture::new().await;
    f.assign(
        vec![subject_permission(
            &f.subject,
            "memory.consolidation.supply",
        )],
        vec![],
    )
    .await;
    if missing_origin {
        let historical = BackendJobRequest {
            job_id: format!("historical-job-{}", Uuid::now_v7()),
            purpose: "subjektiv_consolidation".into(),
            input_ref: format!("subjektiv://{}/consolidation", f.subject),
            input: json!({"subject_id": f.subject, "candidate_ids": []}),
            instruction: "retained legacy input".into(),
            profile: "builtin:subjektiv-memory-consolidation".into(),
            grants: crate::backend_job::BackendJobGrants {
                subjektiv_consolidation: Some(crate::backend_job::SubjektivConsolidationGrant {
                    subject_id: f.subject.clone(),
                    candidate_ids: vec![],
                }),
                ..Default::default()
            },
            serialization_key: None,
            source_worker: None,
            notification_target: None,
            limits: Default::default(),
        };
        let saved = f
            .api
            .store
            .reserve_backend_job(TEST_WORKSPACE_ID, &historical, &Utc::now().to_rfc3339())
            .unwrap();
        return (f._dir, f.api, saved);
    }
    subjektiv_job::dispatch(&f.api, &f.subject, Some(f.source), 0, 0).unwrap();
    let job = f
        .api
        .store
        .find_active_backend_job_for_resource(
            TEST_WORKSPACE_ID,
            &crate::backend_job::subjektiv_serialization_key(&f.subject),
        )
        .unwrap()
        .unwrap();
    let attempt = f
        .api
        .store
        .get_backend_job_attempt(
            TEST_WORKSPACE_ID,
            &job.request.job_id,
            &crate::backend_job::attempt_id(&job.request.job_id, job.current_attempt),
        )
        .unwrap()
        .unwrap();
    (
        f._dir,
        f.api,
        crate::backend_job::BackendJobReservation {
            job,
            attempt,
            replayed: false,
            resource_reused: false,
        },
    )
}

fn replace_policy(api: &WorkspaceApi, subject: &str, operations: &[&str]) {
    let store = open_subjektiv_store(api).unwrap();
    let current = store.subject_permissions(subject).unwrap();
    store
        .set_subject_permissions(
            subject,
            &SubjectPermissionsUpdate {
                operation_id: Uuid::now_v7().to_string(),
                expected_change_id: current.map(|p| p.change_id),
                execution: operations
                    .iter()
                    .map(|op| subject_permission(subject, op))
                    .collect(),
                delegation: vec![],
            },
        )
        .unwrap();
}

async fn model_worker(
    root: &std::path::Path,
    api: &mut WorkspaceApi,
    reservation: &crate::backend_job::BackendJobReservation,
    consolidation: bool,
) -> (worker::Worker<Model, ModelStore>, Model) {
    let target = reservation.attempt.worker.as_ref().unwrap();
    let identity = Arc::new(RuntimeIdentityMaterial::generate(&target.runtime_id).unwrap());
    configure_runtime_request_auth(api, &identity, &target.runtime_id);
    let client = Arc::new(RouterClient {
        api: api.clone(),
        identity,
        worker: target.clone(),
        handle: tokio::runtime::Handle::current(),
        requests: Mutex::new(vec![]),
    });
    let mut manifest = local_task_manifest();
    manifest.worker.name = format!("signed-job-supply-{}", Uuid::now_v7());
    manifest.compaction = Some(manifest::CompactionConfig {
        model: None,
        retained_tokens: 1,
        ..Default::default()
    });
    let model = Model::default();
    let engine =
        agen::Engine::<_, agen::state::Mutable, worker::SessionHistoryMetadata>::new_annotated(
            model.clone(),
        );
    let store = session_store::CombinedStore::new(
        session_store::FsStore::new(root.join("sessions")).unwrap(),
        session_store::FsWorkerStore::new(root.join("workers")).unwrap(),
    );
    let mut worker = worker::Worker::new(
        manifest,
        engine,
        store,
        worker::WorkerWorkspaceContext::with_client(
            Some(worker::WorkspaceId::new(TEST_WORKSPACE_ID).unwrap()),
            client,
        ),
        worker::WorkerFilesystemAuthority::None,
        manifest::Scope::empty(),
    )
    .await
    .unwrap();
    worker
        .bind_backend_job(worker::BackendJobExecutionBinding {
            job_id: reservation.job.request.job_id.clone(),
            attempt_id: reservation.attempt.attempt_id.clone(),
            input_digest: Some(reservation.job.request.input_digest().unwrap()),
            subjektiv_consolidation: consolidation,
            subjektiv_journal: !consolidation,
        })
        .unwrap();
    worker
        .engine_mut()
        .set_system_prompt("Bound Job model fixture");
    (worker, model)
}

struct PolicyRevocation {
    api: WorkspaceApi,
    subject: String,
}
#[async_trait]
impl Hook<PreLlmRequest> for PolicyRevocation {
    async fn call(
        &self,
        _: &PreRequestContext,
    ) -> std::result::Result<HookPreRequestAction, HookError> {
        // Change the real durable policy after the guard's initial signed allow.
        // This observes new-access denial, not Runtime cancellation completion.
        replace_policy(&self.api, &self.subject, &[]);
        Ok(HookPreRequestAction::Continue)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signed_job_host_policy_origin_and_attempt_fences_stop_frozen_input_model_supply() {
    for consolidation in [false, true] {
        for mode in [
            "policy",
            "attempt",
            "origin",
            "late-hook",
            "outside-grant",
            "missing-origin",
        ] {
            let (_dir, mut api, mut reservation) =
                job_fixture(consolidation, mode == "missing-origin").await;
            let subject = reservation
                .job
                .request
                .grants
                .subjektiv_consolidation
                .as_ref()
                .map(|g| g.subject_id.clone())
                .or_else(|| {
                    reservation
                        .job
                        .request
                        .grants
                        .subjektiv_journal
                        .as_ref()
                        .map(|g| g.subject_id.clone())
                })
                .unwrap();
            replace_policy(
                &api,
                &subject,
                &["memory.consolidation.supply", "journal.job.input.supply"],
            );
            if mode == "missing-origin" {
                // Retained pre-migration input through the real ledger. New
                // dispatch rejects this; it must also fail at later model supply.
                let historical = reservation.job.request.clone();
                assert!(historical.source_worker.is_none());
                let now = Utc::now().to_rfc3339();
                let saved = reservation.clone();
                api.store
                    .claim_backend_job_attempt_dispatch(
                        TEST_WORKSPACE_ID,
                        &historical.job_id,
                        &saved.attempt.attempt_id,
                        &now,
                    )
                    .unwrap();
                let target = spawn_ticket_check_source(&api, "historical-job-model");
                api.store
                    .bind_backend_job_attempt_worker(
                        TEST_WORKSPACE_ID,
                        &historical.job_id,
                        &saved.attempt.attempt_id,
                        &target,
                        None,
                        &now,
                    )
                    .unwrap();
                reservation.job = api
                    .store
                    .get_backend_job(TEST_WORKSPACE_ID, &historical.job_id)
                    .unwrap()
                    .unwrap();
                reservation.attempt = api
                    .store
                    .get_backend_job_attempt(
                        TEST_WORKSPACE_ID,
                        &historical.job_id,
                        &saved.attempt.attempt_id,
                    )
                    .unwrap()
                    .unwrap();
            }
            let local = tempfile::tempdir().unwrap();
            let (mut worker, model) = model_worker(
                local.path(),
                &mut api,
                &reservation,
                if mode == "outside-grant" {
                    !consolidation
                } else {
                    consolidation
                },
            )
            .await;
            if mode == "late-hook" {
                worker.add_pre_llm_request_hook(PolicyRevocation {
                    api: api.clone(),
                    subject: subject.clone(),
                });
            }
            let frozen = serde_json::to_string(&reservation.job.request.input).unwrap();
            let outcome = worker.run_text(frozen.clone()).await.unwrap();
            if matches!(mode, "late-hook" | "outside-grant" | "missing-origin") {
                assert!(
                    matches!(outcome, worker::WorkerRunResult::Interrupted { .. }),
                    "{mode}: {outcome:?}"
                );
                assert_eq!(model.calls.load(Ordering::SeqCst), 0, "{mode}");
                continue;
            }
            assert!(
                matches!(outcome, worker::WorkerRunResult::Finished),
                "{mode}: {outcome:?}"
            );
            assert_eq!(model.calls.load(Ordering::SeqCst), 1);
            assert!(
                model.input.lock().unwrap()[0].contains(&subject),
                "allowed real frozen input must reach the model"
            );
            match mode {
                "policy" => replace_policy(&api, &subject, &[]),
                "attempt" => {
                    api.finish_backend_job_attempt(
                        &reservation.job.request.job_id,
                        &reservation.attempt.attempt_id,
                        BackendJobAttemptState::Failed,
                        "cancelled",
                        "test cancellation",
                    )
                    .unwrap();
                }
                "origin" => {
                    // Historical disconnect input in a deterministic execution
                    // fixture, not a claim of live Controller release/cancellation.
                    let source = reservation.job.request.source_worker.as_ref().unwrap();
                    let lease = api
                        .store
                        .require_current_worker_singleton_owner(TEST_WORKSPACE_ID, source)
                        .unwrap()
                        .unwrap();
                    let binding = "historical-disconnected-job-origin";
                    let operation = "historical-job-origin-release";
                    api.config_store
                        .reserve_subject_release(TEST_WORKSPACE_ID, &lease, binding, operation)
                        .unwrap();
                    api.config_store
                        .confirm_subject_release_proof(
                            TEST_WORKSPACE_ID,
                            &lease,
                            binding,
                            operation,
                            false,
                        )
                        .unwrap();
                    api.config_store
                        .release_subject_singleton(TEST_WORKSPACE_ID, &lease, binding, operation)
                        .unwrap();
                }
                _ => unreachable!(),
            }
            let error = worker.compact(0).await.unwrap_err();
            assert!(error.to_string().contains("supply"), "{mode}: {error}");
            assert_eq!(
                model.calls.load(Ordering::SeqCst),
                1,
                "{mode}: frozen input reached compactor after revocation"
            );
            assert_eq!(
                api.store
                    .get_backend_job(TEST_WORKSPACE_ID, &reservation.job.request.job_id)
                    .unwrap()
                    .unwrap()
                    .request,
                reservation.job.request,
                "revocation never rewrites the frozen grant/input/origin"
            );
        }
    }
}
