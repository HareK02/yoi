//! Real Host admission with real Job/domain SQLite stores. Only model responses
//! and the held Job's release timing are substituted; no process-death proof.
use super::*;
use agen::llm_client::{ClientError, LlmClient, Request, event::Event, types::Item};
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

#[derive(Clone, Default)]
struct AdmissionModel {
    clones: Arc<AtomicUsize>,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    submissions: Arc<Mutex<Vec<String>>>,
}
#[async_trait::async_trait]
impl LlmClient for AdmissionModel {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        self.clones.fetch_add(1, Ordering::SeqCst);
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        request: Request,
    ) -> Result<agen::llm_client::client::ResponseStream, ClientError> {
        let envelope = request
            .items
            .iter()
            .filter_map(Item::as_text)
            .find(|text| text.contains("input_json: "))
            .expect("immutable Job input");
        let id = envelope
            .lines()
            .find_map(|line| line.strip_prefix("job_id: "))
            .unwrap();
        let input: serde_json::Value = serde_json::from_str(
            envelope
                .lines()
                .find_map(|line| line.strip_prefix("input_json: "))
                .unwrap(),
        )
        .unwrap();
        let events = if request.items.iter().any(|item| {
            matches!(item,
            Item::ToolResult { call_id, is_error: false, .. } if call_id == "admission-result")
        }) {
            vec![
                Event::text_block_start(0),
                Event::text_delta(0, "done"),
                Event::text_block_stop(0, None),
            ]
        } else {
            {
                let mut submissions = self.submissions.lock().unwrap();
                assert!(
                    !submissions.iter().any(|prior| prior == id),
                    "duplicate execution/model submission"
                );
                submissions.push(id.to_owned());
            }
            if id == "held-capacity" {
                self.entered.notify_one();
                self.release.notified().await;
            }
            vec![
                Event::tool_use_start(0, "admission-result", "SubmitJobResult"),
                Event::tool_input_delta(0, &serde_json::json!({"result": input}).to_string()),
                Event::tool_use_stop(0),
            ]
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}

struct AdmissionFixture {
    body: SubjectConnection,
    jobs: crate::jobs::StandaloneJobs,
    model: AdmissionModel,
    _temp: tempfile::TempDir,
}
impl Drop for AdmissionFixture {
    fn drop(&mut self) {
        self.body.host.live.store(false, Ordering::Release);
    }
}
impl AdmissionFixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let catalog = StandaloneSubjects::open(temp.path().join("state")).unwrap();
        let subject = catalog.create("admission", None).unwrap();
        let body = catalog
            .connect(&subject.id, protocol::WorkerId::now_v7())
            .unwrap();
        let jobs =
            crate::jobs::StandaloneJobs::open(&body.jobs_path, temp.path().to_path_buf()).unwrap();
        let model = AdmissionModel::default();
        jobs.bind_model_client(Box::new(model.clone()));
        body.host.bind_jobs(jobs.clone()).unwrap();
        Self {
            body,
            jobs,
            model,
            _temp: temp,
        }
    }
    fn reserve(&self) -> crate::jobs::JobSnapshot {
        self.reserve_with_profile("builtin:standalone-subjektiv-consolidation")
    }
    fn reserve_with_profile(&self, profile: &str) -> crate::jobs::JobSnapshot {
        self.jobs.request_granted(job::JobRequest {
            job_id: "reserved-consolidation".into(), purpose: "subjektiv_consolidation".into(),
            input_ref: format!("subjektiv://{}/consolidation", self.body.binding.subject_id),
            input: serde_json::json!({"subject_id": self.body.binding.subject_id, "candidate_ids": []}),
            instruction: "Return the exact immutable input with SubmitJobResult".into(),
            profile: profile.into(),
            serialization_key: Some(format!("subjektiv:{}", self.body.binding.subject_id)),
            limits: Default::default(),
        }, serde_json::to_value(LocalConsolidationGrant {
            binding: self.body.binding.clone(), candidate_ids: vec![],
        }).unwrap()).unwrap()
    }
    fn domain_path(&self) -> PathBuf {
        self.body
            .host
            .catalog
            .root
            .join("features")
            .join(&self.body.host.catalog.scope_id)
            .join("features/subjektiv.sqlite")
    }
    async fn hold_capacity(&self) {
        self.jobs
            .request(job::JobRequest {
                job_id: "held-capacity".into(),
                purpose: "test".into(),
                input_ref: "fixture:held".into(),
                input: serde_json::json!({"held": true}),
                instruction: "Return the input".into(),
                profile: "builtin:job".into(),
                serialization_key: None,
                limits: job::JobLimits {
                    max_concurrent_jobs: 1,
                    ..Default::default()
                },
            })
            .unwrap();
        self.jobs.start("held-capacity").await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), self.model.entered.notified())
            .await
            .unwrap();
        let held = self.jobs.get("held-capacity").unwrap();
        assert_eq!(held.attempt.state, crate::jobs::JobAttemptState::Dispatched);
        assert!(!held.executions_released());
    }
    async fn assert_completed_once(&self, before: &crate::jobs::JobSnapshot) {
        let id = &before.request.job_id;
        until(
            "same reserved consolidation must progress after ONLY fault/capacity release",
            || {
                self.jobs.get(id).is_ok_and(|saved| {
                    saved.state == crate::jobs::JobState::Completed
                        && saved.acknowledged
                        && saved.executions_released()
                })
            },
        )
        .await;
        let after = self.jobs.wait(id).await.unwrap();
        assert_eq!(after.request, before.request);
        assert_eq!(after.input_digest, before.input_digest);
        assert_eq!(after.domain_grant, before.domain_grant);
        assert_eq!(after.attempt.attempt_id, before.attempt.attempt_id);
        assert_eq!(after.attempt.number, before.attempt.number);
        assert_eq!(after.executions.len(), 1);
        assert!(after.result.as_ref().unwrap().get("surface").is_some());
        assert_eq!(
            self.model
                .submissions
                .lock()
                .unwrap()
                .iter()
                .filter(|prior| *prior == id)
                .count(),
            1
        );
        let job_db = rusqlite::Connection::open(&self.body.jobs_path).unwrap();
        let attempts: i64 = job_db
            .query_row(
                "SELECT count(*) FROM job_attempts WHERE job_id=?1",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(attempts, 1);
        let domain = rusqlite::Connection::open(self.domain_path()).unwrap();
        let generations: i64 = domain
            .query_row(
                "SELECT count(*) FROM surface_generation_runs WHERE job_id=?1 AND attempt_id=?2",
                rusqlite::params![id, before.attempt.attempt_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(generations, 1, "finalizer must prepare exactly once");
        assert_eq!(
            subjektiv::job::lookup_result(
                &self.body.host.catalog.store,
                &self.body.binding.subject_id,
                &[],
                id,
                &before.attempt.attempt_id
            )
            .unwrap(),
            after.result
        );
    }
}
async fn until(message: &str, mut check: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect(message);
}

#[tokio::test]
async fn consolidation_admission_capacity_only_release_starts_original_request() {
    capacity_only_release(false).await;
}
#[tokio::test]
async fn consolidation_admission_capacity_only_release_starts_restored_reservation() {
    capacity_only_release(true).await;
}
async fn capacity_only_release(restore: bool) {
    let fixture = AdmissionFixture::new();
    fixture.hold_capacity().await;
    let clones = fixture.model.clones.load(Ordering::SeqCst);
    let before = if restore {
        let before = fixture.reserve();
        fixture.body.host.recover_jobs();
        before
    } else {
        let result = fixture
            .body
            .host
            .request_consolidation(
                &SubjektivHostContext {
                    worker_id: fixture.body.host.worker_key.clone(),
                    session_id: session_store::new_session_id().to_string(),
                },
                memory::backend::MemoryConsolidateStagingOperation { force: true },
            )
            .unwrap();
        assert_eq!(result.status, "started");
        fixture
            .jobs
            .pending()
            .unwrap()
            .into_iter()
            .find(|saved| saved.domain_grant.is_some())
            .unwrap()
    };
    // Preparation clones the configured transport before checking actual capacity.
    // This barrier proves the production scheduler attempted admission while held.
    until("scheduler must reach capacity check", || {
        fixture.model.clones.load(Ordering::SeqCst) > clones
    })
    .await;
    assert_eq!(fixture.jobs.get(&before.request.job_id).unwrap(), before);
    assert!(before.executions.is_empty());
    assert!(matches!(
        fixture.jobs.start(&before.request.job_id).await,
        Err(crate::jobs::JobServiceError::Capacity)
    ));
    fixture.model.release.notify_one();
    let held = tokio::time::timeout(Duration::from_secs(10), fixture.jobs.wait("held-capacity"))
        .await
        .unwrap()
        .unwrap();
    assert!(held.executions_released());
    fixture.assert_completed_once(&before).await;
}

#[tokio::test]
async fn consolidation_admission_reserved_domain_busy_release_before_fast_budget() {
    reserved_domain_busy_release(false).await;
}
#[tokio::test]
async fn consolidation_admission_reserved_domain_busy_release_beyond_fast_budget() {
    reserved_domain_busy_release(true).await;
}
async fn reserved_domain_busy_release(beyond: bool) {
    let fixture = AdmissionFixture::new();
    let before = fixture.reserve();
    let connection = rusqlite::Connection::open(fixture.domain_path()).unwrap();
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    // Production restore entry, not a manually armed retry or failed-attempt fixture.
    fixture.body.host.recover_jobs();
    let required_passes = if beyond { 8 } else { 1 };
    // Real SQLite busy_timeout makes eight attempts longer than the short wait.
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let owners = fixture.body.host.consolidation_recovery.lock().unwrap();
            if let Some(owner) = owners
                .values()
                .find(|owner| owner.passes >= required_passes)
            {
                super::assert_real_busy(&owner.error, true);
                if beyond {
                    assert_eq!(
                        owner.progress,
                        ConsolidationRecoveryProgress::RetainedStorage
                    );
                }
                break;
            }
            drop(owners);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("restore must own the actual reserved-domain Busy operation");
    assert_eq!(fixture.jobs.get(&before.request.job_id).unwrap(), before);
    assert!(fixture.model.submissions.lock().unwrap().is_empty());
    eprintln!(
        "D2 actual reserved Feature Busy retained; beyond_fast_budget={beyond}; rollback ONLY"
    );
    connection.execute_batch("ROLLBACK").unwrap();
    // No restore/recover_jobs/reclick/new request after releasing the SQLite fault.
    fixture.assert_completed_once(&before).await;
}

async fn waiting_capacity(fixture: &AdmissionFixture) {
    until(
        "exact admission must own the actual capacity refusal",
        || {
            fixture
                .body
                .host
                .consolidation_admissions
                .lock()
                .unwrap()
                .values()
                .any(|owner| {
                    matches!(
                        owner.issue,
                        Some(ConsolidationRecoveryError::Service(
                            crate::jobs::JobServiceError::Capacity
                        ))
                    )
                })
        },
    )
    .await;
}

#[tokio::test]
async fn consolidation_admission_duplicate_notifications_preserve_deadline_and_never_forge_cleanup()
{
    let fixture = AdmissionFixture::new();
    fixture.hold_capacity().await;
    let before = fixture.reserve();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
    schedule_job(
        fixture.jobs.clone(),
        before.clone(),
        fixture.body.host.self_weak.clone(),
        Some(deadline),
    );
    waiting_capacity(&fixture).await;
    // A late notification carrying a fresh timeout must not reset the original.
    schedule_job(
        fixture.jobs.clone(),
        before.clone(),
        fixture.body.host.self_weak.clone(),
        Some(deadline + Duration::from_secs(60)),
    );
    {
        let owners = fixture.body.host.consolidation_admissions.lock().unwrap();
        assert_eq!(owners.len(), 1);
        assert_eq!(owners.values().next().unwrap().deadline, deadline);
    }
    until(
        "the original admission deadline must persist a specific pre-dispatch failure",
        || {
            fixture.jobs.get(&before.request.job_id).unwrap().state == crate::jobs::JobState::Failed
                && fixture
                    .body
                    .host
                    .consolidation_admissions
                    .lock()
                    .unwrap()
                    .is_empty()
        },
    )
    .await;
    let failed = fixture.jobs.get(&before.request.job_id).unwrap();
    assert!(
        failed
            .attempt
            .failure
            .as_deref()
            .unwrap()
            .contains("consolidation_admission_deadline_exceeded")
    );
    assert_eq!(failed.attempt.attempt_id, before.attempt.attempt_id);
    assert_eq!(failed.domain_grant, before.domain_grant);
    assert_eq!(failed.input_digest, before.input_digest);
    assert!(failed.executions.is_empty());
    assert!(failed.start_rejected_by.is_some());
    assert!(
        !fixture
            .jobs
            .get("held-capacity")
            .unwrap()
            .executions_released(),
        "refusal must not invent prior cleanup"
    );
    assert_eq!(
        *fixture.model.submissions.lock().unwrap(),
        ["held-capacity"]
    );
    fixture.model.release.notify_one();
    assert!(
        fixture
            .jobs
            .wait("held-capacity")
            .await
            .unwrap()
            .executions_released()
    );
    assert_eq!(fixture.jobs.get(&before.request.job_id).unwrap(), failed);
}

#[tokio::test]
async fn consolidation_admission_capacity_wait_rechecks_retirement_binding_and_version_stop() {
    for case in ["erased", "version_stopped", "inactive", "released_writer"] {
        let fixture = AdmissionFixture::new();
        fixture.hold_capacity().await;
        let before = fixture.reserve();
        fixture.body.host.recover_jobs();
        waiting_capacity(&fixture).await;
        match case {
            "erased" => {
                fixture
                    .jobs
                    .reclaim_input(
                        &before.request.job_id,
                        &before.input_digest,
                        crate::jobs::JobInputRetirementReason::ExplicitErasure,
                    )
                    .unwrap();
            }
            "version_stopped" => {
                rusqlite::Connection::open(&fixture.body.jobs_path)
                    .unwrap()
                    .execute(
                        "INSERT INTO job_version_stops(job_id) VALUES(?1)",
                        [&before.request.job_id],
                    )
                    .unwrap();
            }
            "inactive" => fixture.body.host.live.store(false, Ordering::Release),
            "released_writer" => fixture
                .body
                .host
                .catalog
                .store
                .release_journal_writer(&fixture.body.host.writer)
                .unwrap(),
            _ => unreachable!(),
        }
        let expected = fixture.jobs.get(&before.request.job_id).unwrap();
        fixture.model.release.notify_one();
        fixture.jobs.wait("held-capacity").await.unwrap();
        until(
            "capacity wake must retain the specific refusal without dispatch",
            || {
                fixture
                    .body
                    .host
                    .consolidation_admissions
                    .lock()
                    .unwrap()
                    .values()
                    .any(|owner| owner.blocked)
            },
        )
        .await;
        assert_eq!(
            fixture.jobs.get(&before.request.job_id).unwrap(),
            expected,
            "{case}"
        );
        assert!(expected.executions.is_empty(), "{case}");
        assert_eq!(
            *fixture.model.submissions.lock().unwrap(),
            ["held-capacity"],
            "{case}"
        );
    }
}

#[tokio::test]
async fn consolidation_admission_stale_attempt_input_or_grant_never_starts_current_reservation() {
    for case in ["attempt", "digest", "grant"] {
        let fixture = AdmissionFixture::new();
        let before = fixture.reserve();
        let mut stale = before.clone();
        match case {
            "attempt" => stale.attempt.attempt_id = "different-attempt".into(),
            "digest" => stale.input_digest = "different-digest".into(),
            "grant" => stale.domain_grant = Some(serde_json::json!({"foreign":true})),
            _ => unreachable!(),
        }
        schedule_job(
            fixture.jobs.clone(),
            stale,
            fixture.body.host.self_weak.clone(),
            None,
        );
        until(
            "stale exact admission must be diagnosed, never followed by ID",
            || {
                fixture
                    .body
                    .host
                    .consolidation_admissions
                    .lock()
                    .unwrap()
                    .values()
                    .any(|owner| owner.blocked)
            },
        )
        .await;
        assert_eq!(
            fixture.jobs.get(&before.request.job_id).unwrap(),
            before,
            "{case}"
        );
        assert!(
            fixture.model.submissions.lock().unwrap().is_empty(),
            "{case}"
        );
    }
}

#[tokio::test]
async fn consolidation_admission_shutdown_releases_waiter_without_dispatch_or_cleanup_claim() {
    let fixture = AdmissionFixture::new();
    fixture.hold_capacity().await;
    let before = fixture.reserve();
    fixture.body.host.recover_jobs();
    waiting_capacity(&fixture).await;
    // Real service shutdown closes admission and signals the actual held runner.
    fixture.jobs.close();
    until("shutdown must release its volatile admission owner", || {
        fixture
            .body
            .host
            .consolidation_admissions
            .lock()
            .unwrap()
            .is_empty()
    })
    .await;
    assert_eq!(fixture.jobs.get(&before.request.job_id).unwrap(), before);
    assert_eq!(
        *fixture.model.submissions.lock().unwrap(),
        ["held-capacity"]
    );
    fixture.model.release.notify_one();
    tokio::time::timeout(Duration::from_secs(10), fixture.jobs.wait("held-capacity"))
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn consolidation_admission_permanent_preparation_refusal_is_saved_without_execution() {
    let fixture = AdmissionFixture::new();
    // A real existing Profile that does not satisfy consolidation's explicit
    // policy requirement; no fake transport/storage error stands in for refusal.
    let before = fixture.reserve_with_profile("builtin:job");
    fixture.body.host.recover_jobs();
    until(
        "permanent preparation refusal must become a specific durable failure",
        || {
            fixture.jobs.get(&before.request.job_id).unwrap().state == crate::jobs::JobState::Failed
                && fixture
                    .body
                    .host
                    .consolidation_admissions
                    .lock()
                    .unwrap()
                    .is_empty()
        },
    )
    .await;
    let failed = fixture.jobs.get(&before.request.job_id).unwrap();
    assert!(
        failed
            .attempt
            .failure
            .as_deref()
            .unwrap()
            .contains("consolidation_admission_cannot_start")
    );
    assert!(failed.start_rejected_by.is_some());
    assert!(failed.executions.is_empty());
    assert_eq!(failed.request, before.request);
    assert_eq!(failed.domain_grant, before.domain_grant);
    assert_eq!(failed.input_digest, before.input_digest);
    assert_eq!(failed.attempt.attempt_id, before.attempt.attempt_id);
    assert!(fixture.model.submissions.lock().unwrap().is_empty());
    assert!(
        crate::job_store::JobStore::open(&fixture.body.jobs_path)
            .unwrap()
            .execution_obligations()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn consolidation_admission_reserved_storage_handoff_preserves_deadline_while_capacity_held() {
    let fixture = AdmissionFixture::new();
    fixture.hold_capacity().await;
    let before = fixture.reserve();
    let connection = rusqlite::Connection::open(fixture.domain_path()).unwrap();
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    fixture.body.host.recover_jobs();
    let deadline = {
        let owners = fixture.body.host.consolidation_recovery.lock().unwrap();
        assert_eq!(owners.len(), 1);
        let owner = owners.values().next().unwrap();
        super::assert_real_busy(&owner.error, true);
        owner.admission_deadline
    };
    connection.execute_batch("ROLLBACK").unwrap();
    waiting_capacity(&fixture).await;
    {
        let owners = fixture.body.host.consolidation_admissions.lock().unwrap();
        assert_eq!(owners.len(), 1);
        assert_eq!(owners.values().next().unwrap().deadline, deadline);
    }
    // Duplicate restoration while capacity remains held cannot reset the budget.
    fixture.body.host.recover_jobs();
    assert_eq!(
        fixture
            .body
            .host
            .consolidation_admissions
            .lock()
            .unwrap()
            .values()
            .next()
            .unwrap()
            .deadline,
        deadline
    );
    assert_eq!(fixture.jobs.get(&before.request.job_id).unwrap(), before);
    fixture.model.release.notify_one();
    fixture.jobs.wait("held-capacity").await.unwrap();
    fixture.assert_completed_once(&before).await;
}

#[tokio::test]
async fn consolidation_admission_missing_prior_cleanup_never_dispatches_or_creates_release_proof() {
    let fixture = super::ConsolidationRecoveryFixture::new();
    let before = fixture.saved.clone();
    assert!(!before.executions_released());
    schedule_job(
        fixture.jobs.clone(),
        before.clone(),
        fixture.body.host.self_weak.clone(),
        None,
    );
    until(
        "non-reserved work must leave admission without replay",
        || {
            fixture
                .body
                .host
                .consolidation_admissions
                .lock()
                .unwrap()
                .is_empty()
        },
    )
    .await;
    assert_eq!(fixture.jobs.get(&before.request.job_id).unwrap(), before);
    assert!(fixture.domain_closure().is_none());
    assert!(
        !fixture
            .jobs
            .get(&before.request.job_id)
            .unwrap()
            .executions_released()
    );
}
