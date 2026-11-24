//! Consumer deletion fences use the common Job settlement path. Runtime execution
//! is fixture-owned; native Subject/Journal and Backend SQLite state remain real.
use super::*;
use crate::backend_job::JobResultApplicationDisposition;

#[tokio::test]
async fn deleted_subject_before_dispatch_records_omission_without_model_success() {
    let f = Fixture::new().await;
    f.api.stop_backend_job_recovery().await;
    let subject = &f
        .request
        .grants
        .subjektiv_journal
        .as_ref()
        .unwrap()
        .subject_id;
    f.store
        .begin_subject_deletion(subject, "application-target-deleted")
        .unwrap();
    f.execution.take_inputs();
    f.api
        .dispatch_backend_job_reservation(f.reserved.clone())
        .unwrap();
    let attempt = f
        .api
        .store
        .get_backend_job_attempt(
            TEST_WORKSPACE_ID,
            &f.request.job_id,
            &f.reserved.attempt.attempt_id,
        )
        .unwrap()
        .unwrap();
    assert_eq!(attempt.state, BackendJobAttemptState::Failed);
    assert_eq!(
        attempt.result_application_disposition,
        Some(JobResultApplicationDisposition::NotRequired)
    );
    assert!(attempt.result.is_none());
    assert!(attempt.worker.is_none());
    assert!(f.execution.take_inputs().is_empty());
    assert!(f.store.subject(subject).unwrap().is_none());
    assert!(f.store.subject_deletion(subject).unwrap().is_some());
}

#[tokio::test]
async fn erased_session_before_dispatch_is_not_required_but_other_session_identity_is_not() {
    let f = Fixture::new().await;
    f.api.stop_backend_job_recovery().await;
    assert!(!backend_job_application::target_not_required(&f.api, &f.request).unwrap());
    f.store
        .erase_journal_session(&f.host, "application-session-erased")
        .unwrap();
    assert!(backend_job_application::target_not_required(&f.api, &f.request).unwrap());
    let mut wrong = f.request.clone();
    wrong.grants.subjektiv_journal.as_mut().unwrap().binding_id = "different-lifetime".into();
    assert!(backend_job_application::target_not_required(&f.api, &wrong).is_err());
    f.api
        .dispatch_backend_job_reservation(f.reserved.clone())
        .unwrap();
    let attempt = f
        .api
        .store
        .get_backend_job_attempt(
            TEST_WORKSPACE_ID,
            &f.request.job_id,
            &f.reserved.attempt.attempt_id,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        attempt.result_application_disposition,
        Some(JobResultApplicationDisposition::NotRequired)
    );
    assert!(attempt.result.is_none());
    assert!(attempt.worker.is_none());
}

#[tokio::test]
async fn missing_subject_without_canonical_deletion_is_not_normal_omission() {
    let f = Fixture::new().await;
    f.api.stop_backend_job_recovery().await;
    let mut request = f.request.clone();
    request
        .grants
        .subjektiv_journal
        .as_mut()
        .unwrap()
        .subject_id = "never-admitted-subject".into();
    assert!(matches!(
        backend_job_application::target_not_required(&f.api, &request),
        Err(Error::SubjektivSubjectNotFound(_))
    ));
}

#[tokio::test]
async fn consolidation_consumes_same_deletion_boundary_without_journal_state() {
    let f = Fixture::new().await;
    f.api.stop_backend_job_recovery().await;
    let subject = f
        .request
        .grants
        .subjektiv_journal
        .as_ref()
        .unwrap()
        .subject_id
        .clone();
    let mut request = f.request.clone();
    request.job_id = "consolidation-omission".into();
    request.purpose = "subjektiv_consolidation".into();
    request.grants.subjektiv_journal = None;
    request.grants.subjektiv_consolidation =
        Some(crate::backend_job::SubjektivConsolidationGrant {
            subject_id: subject.clone(),
            candidate_ids: Vec::new(),
        });
    request.serialization_key = Some(crate::backend_job::subjektiv_serialization_key(&subject));
    request.profile = "builtin:job".into();
    let reservation = f
        .api
        .store
        .reserve_backend_job(TEST_WORKSPACE_ID, &request, &Utc::now().to_rfc3339())
        .unwrap();
    assert!(!backend_job_application::target_not_required(&f.api, &request).unwrap());
    f.store
        .begin_subject_deletion(&subject, "consolidation-target-deleted")
        .unwrap();
    assert!(backend_job_application::target_not_required(&f.api, &request).unwrap());
    f.api
        .dispatch_backend_job_reservation(reservation.clone())
        .unwrap();
    let attempt = f
        .api
        .store
        .get_backend_job_attempt(
            TEST_WORKSPACE_ID,
            &request.job_id,
            &reservation.attempt.attempt_id,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        attempt.result_application_disposition,
        Some(JobResultApplicationDisposition::NotRequired)
    );
    assert_eq!(attempt.state, BackendJobAttemptState::Failed);
    assert!(attempt.result.is_none());
}

/// Uses the existing real-store fixture, but retires its unused reservation so
/// only the finite Host Job can create prefixes. Execution remains deterministic.
struct RecordingFixture {
    fixture: Fixture,
    job_id: String,
    chunks: Vec<session_store::capture::CaptureDto>,
}

impl RecordingFixture {
    async fn new(split: bool) -> Self {
        use session_store::capture::RecordingPosition;
        let fixture = Fixture::new().await;
        fixture.api.stop_backend_job_recovery().await;
        fixture
            .api
            .finish_backend_job_attempt(
                &fixture.request.job_id,
                &fixture.reserved.attempt.attempt_id,
                BackendJobAttemptState::Failed,
                "unused_fixture_reservation",
                "No allocation: recording Host Job owns subsequent prefixes",
            )
            .unwrap();
        fixture
            .api
            .finish_backend_job_cleanup_retirement(
                &fixture.request.job_id,
                &fixture.reserved.attempt.attempt_id,
            )
            .unwrap();
        // Both fragments retain the original descriptor/digest. Their concatenated
        // body is exactly the source used by Fixture; no synthetic success result.
        let text = "original public input";
        let chunks = if split {
            let mut tail = fixture.range.clone();
            tail.bytes.start = 8;
            vec![
                fixture_capture(&fixture.range, &text[..8]),
                fixture_capture(&tail, &text[8..]),
            ]
        } else {
            vec![fixture_capture(&fixture.range, text)]
        };
        let grant = fixture.request.grants.subjektiv_journal.as_ref().unwrap();
        let job_id = backend_recording_job::admit(
            &fixture.api,
            crate::store::BackendJobRecordingScope {
                subject_id: grant.subject_id.clone(),
                session_id: grant.session_id.clone(),
                worker: fixture.source.clone(),
                binding_id: grant.binding_id.clone(),
                database: fixture
                    .api
                    .feature_storage
                    .existing_identity(&fixture.api.subjektiv_registration)
                    .unwrap(),
                start: RecordingPosition::default(),
                end: RecordingPosition {
                    canonical_order: 1,
                    byte_offset: 0,
                },
            },
            &chunks,
        )
        .unwrap();
        // Drain the admission's stopped owner before driving individual steps.
        fixture.api.stop_backend_job_recovery().await;
        fixture.execution.take_inputs();
        Self {
            fixture,
            job_id,
            chunks,
        }
    }

    fn reopened_api(&self) -> WorkspaceApi {
        let reopened = Arc::new(
            SqliteWorkspaceStore::open_for_test(&self.fixture.api.config.database_path).unwrap(),
        );
        let mut api = self.fixture.api.clone();
        api.store = reopened.clone();
        api.config_store = reopened;
        api
    }

    fn assert_saved_chunks(&self, api: &WorkspaceApi) {
        let page = api
            .config_store
            .get_backend_job_recording_chunks(TEST_WORKSPACE_ID, &self.job_id, 0, 16)
            .unwrap();
        assert!(page.next_index.is_none());
        assert_eq!(
            page.chunks
                .into_iter()
                .map(|c| c.capture)
                .collect::<Vec<_>>(),
            self.chunks
        );
        let job = api
            .store
            .get_backend_job(TEST_WORKSPACE_ID, &self.job_id)
            .unwrap()
            .unwrap();
        assert!(job.request.source_worker.is_none());
        assert!(
            api.store
                .list_backend_job_attempts(TEST_WORKSPACE_ID, &self.job_id)
                .unwrap()
                .is_empty()
        );
    }

    fn child(&self, api: &WorkspaceApi) -> crate::backend_job::BackendJobRecord {
        let input = api
            .config_store
            .get_backend_job_recording_input(TEST_WORKSPACE_ID, &self.job_id)
            .unwrap()
            .unwrap();
        api.store
            .get_backend_job(
                TEST_WORKSPACE_ID,
                input.current_child_job_id.as_deref().unwrap(),
            )
            .unwrap()
            .unwrap()
    }

    fn dispatch_child(&self, api: &WorkspaceApi) -> BackendJobReservation {
        let child = self.child(api);
        let reservation = api
            .store
            .reserve_backend_job(TEST_WORKSPACE_ID, &child.request, &Utc::now().to_rfc3339())
            .unwrap();
        api.dispatch_backend_job_reservation(reservation).unwrap()
    }

    fn checkpoint(&self, api: &WorkspaceApi, dispatched: &BackendJobReservation) {
        let worker = dispatched.attempt.worker.as_ref().unwrap();
        let input =
            subjektiv::journal::recording::JournalInput::decode(&dispatched.job.request.input)
                .unwrap();
        let record_id = format!("{}:record", dispatched.attempt.attempt_id);
        let JournalJobResponse::Saved(receipt) = subjektiv_journal::execute(
            api,
            worker,
            JournalJobRequest::Execute {
                operation: JournalOperation {
                    request_id: record_id.clone(),
                    expected_state: input.initial_state_token,
                    action: JournalAction::ConfirmUnchanged,
                    covered_ranges: input.selected_ranges.clone(),
                },
            },
        )
        .unwrap() else {
            panic!("record receipt required")
        };
        let JournalJobResponse::Saved(position) = subjektiv_journal::execute(
            api,
            worker,
            JournalJobRequest::Execute {
                operation: JournalOperation {
                    request_id: format!("{}:position", dispatched.attempt.attempt_id),
                    expected_state: receipt.state_token,
                    action: JournalAction::UpdatePosition {
                        current_position: "Recording finite prefix".into(),
                    },
                    covered_ranges: vec![],
                },
            },
        )
        .unwrap() else {
            panic!("position receipt required")
        };
        subjektiv_journal::execute(
            api,
            worker,
            JournalJobRequest::Execute {
                operation: JournalOperation {
                    request_id: format!("{}:checkpoint", dispatched.attempt.attempt_id),
                    expected_state: position.state_token,
                    action: JournalAction::Checkpoint {
                        record_operation_ids: vec![record_id],
                    },
                    covered_ranges: input.selected_ranges,
                },
            },
        )
        .unwrap();
        let completion = subjektiv_journal::finish(api, worker).unwrap();
        api.accept_backend_job_result(
            worker,
            &BackendJobResultSubmission {
                job_id: dispatched.job.request.job_id.clone(),
                attempt_id: dispatched.attempt.attempt_id.clone(),
                input_digest: dispatched.attempt.input_digest.clone(),
                result: serde_json::to_value(completion).unwrap(),
            },
        )
        .unwrap();
    }
}

async fn erased_recording_target_is_not_required(delete_subject: bool) {
    let f = RecordingFixture::new(false).await;
    let api = &f.fixture.api;
    f.assert_saved_chunks(api);
    let grant = f.fixture.request.grants.subjektiv_journal.as_ref().unwrap();
    if delete_subject {
        f.fixture
            .store
            .begin_subject_deletion(&grant.subject_id, "finite-target-deleted")
            .unwrap();
    } else {
        f.fixture
            .store
            .erase_journal_session(&f.fixture.host, "finite-session-erased")
            .unwrap();
    }
    let registry = api
        .store
        .get_worker_registry(TEST_WORKSPACE_ID, &f.fixture.source)
        .unwrap();
    assert!(backend_recording_job::step(api, &f.job_id).unwrap());
    let settled = api
        .store
        .get_backend_job(TEST_WORKSPACE_ID, &f.job_id)
        .unwrap()
        .unwrap();
    assert_eq!(settled.state, BackendJobState::Completed);
    assert_eq!(
        settled.result,
        Some(serde_json::json!({"application":"not_required"}))
    );
    let reopened = f.reopened_api();
    for _ in 0..2 {
        assert!(backend_recording_job::step(&reopened, &f.job_id).unwrap());
    }
    assert_eq!(
        reopened
            .store
            .get_backend_job(TEST_WORKSPACE_ID, &f.job_id)
            .unwrap()
            .unwrap(),
        settled
    );
    f.assert_saved_chunks(&reopened);
    assert!(
        reopened
            .config_store
            .list_backend_job_recording_child_ids(TEST_WORKSPACE_ID, &f.job_id, None, 100,)
            .unwrap()
            .is_empty()
    );
    assert!(f.fixture.execution.take_inputs().is_empty());
    assert_eq!(
        reopened
            .store
            .get_worker_registry(TEST_WORKSPACE_ID, &f.fixture.source)
            .unwrap(),
        registry
    );
    let domain = open_subjektiv_store(&reopened).unwrap();
    if delete_subject {
        assert!(domain.subject(&grant.subject_id).unwrap().is_none());
        assert!(
            domain
                .subject_deletion(&grant.subject_id)
                .unwrap()
                .is_some()
        );
    } else {
        assert!(domain.journal_session_erased(&f.fixture.host).unwrap());
        assert!(domain.read_journal(&f.fixture.host, &f.fixture.id).is_err());
    }
}

#[tokio::test]
async fn finite_recording_waits_for_accepted_prefix_and_settles_failed_dependency_without_losing_bytes()
 {
    use session_store::capture::RecordingPosition;
    let f = RecordingFixture::new(true).await;
    let api = &f.fixture.api;
    let mut scope = api
        .config_store
        .get_backend_job_recording_input(TEST_WORKSPACE_ID, &f.job_id)
        .unwrap()
        .unwrap()
        .scope;
    scope.start = RecordingPosition {
        canonical_order: 0,
        byte_offset: 8,
    };
    let tail_id = backend_recording_job::admit(api, scope, &f.chunks[1..]).unwrap();
    api.stop_backend_job_recovery().await;
    let original = api
        .config_store
        .get_backend_job_recording_chunks(TEST_WORKSPACE_ID, &tail_id, 0, 16)
        .unwrap();
    for _ in 0..2 {
        assert!(
            !backend_recording_job::step(api, &tail_id).unwrap(),
            "an accepted unfinished prefix is a dependency wait, not a conflict"
        );
    }
    assert!(
        api.config_store
            .list_backend_job_recording_child_ids(TEST_WORKSPACE_ID, &tail_id, None, 100)
            .unwrap()
            .is_empty()
    );
    api.config_store
        .finish_backend_job_recording(
            TEST_WORKSPACE_ID,
            &f.job_id,
            &crate::store::BackendJobRecordingOutcome::Failed {
                category: "fixture_prefix_failure".into(),
                detail: "No child or external effects were started".into(),
            },
            &Utc::now().to_rfc3339(),
        )
        .unwrap();
    let mut legacy = f.fixture.request.clone();
    legacy.job_id = "legacy-prefix-continuation".into();
    legacy.limits.max_attempts = 2;
    let legacy = api
        .store
        .reserve_backend_job(TEST_WORKSPACE_ID, &legacy, &Utc::now().to_rfc3339())
        .unwrap();
    api.finish_backend_job_attempt(
        &legacy.job.request.job_id,
        &legacy.attempt.attempt_id,
        BackendJobAttemptState::Failed,
        "journal_attempt_fenced",
        "No Worker allocated",
    )
    .unwrap();
    api.finish_backend_job_cleanup_retirement(
        &legacy.job.request.job_id,
        &legacy.attempt.attempt_id,
    )
    .unwrap();
    let eligible = api
        .store
        .get_backend_job(TEST_WORKSPACE_ID, &legacy.job.request.job_id)
        .unwrap()
        .unwrap();
    assert!(subjektiv_journal::can_resume_partial(&eligible));
    // Cleanup release must not close the recovery window before the legacy
    // owner reserves its next attempt. No Runtime termination is fabricated.
    assert!(!backend_recording_job::step(api, &tail_id).unwrap());
    assert!(subjektiv_journal::resume_partial_job(api, &legacy.job.request.job_id).unwrap());
    let retry = api
        .store
        .get_backend_job(TEST_WORKSPACE_ID, &legacy.job.request.job_id)
        .unwrap()
        .unwrap();
    assert_eq!(retry.current_attempt, 2);
    assert!(!backend_recording_job::step(api, &tail_id).unwrap());
    let attempt = api
        .store
        .list_backend_job_attempts(TEST_WORKSPACE_ID, &legacy.job.request.job_id)
        .unwrap()
        .pop()
        .unwrap();
    api.finish_backend_job_attempt(
        &legacy.job.request.job_id,
        &attempt.attempt_id,
        BackendJobAttemptState::Failed,
        "deadline_exceeded",
        "No Worker allocated",
    )
    .unwrap();
    api.finish_backend_job_cleanup_retirement(&legacy.job.request.job_id, &attempt.attempt_id)
        .unwrap();
    let reopened = f.reopened_api();
    assert!(backend_recording_job::step(&reopened, &tail_id).unwrap());
    let failed = reopened
        .store
        .get_backend_job(TEST_WORKSPACE_ID, &tail_id)
        .unwrap()
        .unwrap();
    assert_eq!(failed.state, BackendJobState::Failed);
    assert_eq!(
        failed.failure_category.as_deref(),
        Some("recording_prefix_failed")
    );
    assert!(failed.result.is_none());
    assert!(backend_recording_job::step(&reopened, &tail_id).unwrap());
    assert_eq!(
        reopened
            .store
            .get_backend_job(TEST_WORKSPACE_ID, &tail_id)
            .unwrap()
            .unwrap(),
        failed
    );
    assert_eq!(
        reopened
            .config_store
            .get_backend_job_recording_chunks(TEST_WORKSPACE_ID, &tail_id, 0, 16)
            .unwrap(),
        original
    );
    f.assert_saved_chunks(&reopened);
    assert!(f.fixture.execution.take_inputs().is_empty());
    let journal = f
        .fixture
        .store
        .read_journal(&f.fixture.host, &f.fixture.id)
        .unwrap();
    assert_eq!((journal.process_order, journal.process_byte), (0, 0));
}

#[tokio::test]
async fn finite_recording_recovery_waits_outside_host_lane_and_shutdown_rechecks_admission() {
    let f = RecordingFixture::new(false).await;
    let api = &f.fixture.api;
    let gate = api.backend_job_recovery.pause_steps_for_test().await;
    api.backend_job_recovery.resume_after_test_pause();
    backend_recording_job::recover(api).unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        api.backend_job_recovery
            .recording_before_admission
            .notified(),
    )
    .await
    .unwrap();
    // Snapshot actual accepted Host work. A recording step bypassing the gate
    // would have created its first child before this drain completes.
    api.host_executors.host().unwrap().drain().await.unwrap();
    let children = api
        .config_store
        .list_backend_job_recording_child_ids(TEST_WORKSPACE_ID, &f.job_id, None, 100)
        .unwrap();
    api.backend_job_recovery
        .stopping
        .store(true, Ordering::Release);
    drop(gate);
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        api.stop_backend_job_recovery(),
    )
    .await
    .unwrap();
    assert!(
        children.is_empty(),
        "recording recovery must share ordinary Job admission before entering Host"
    );
    assert!(
        api.config_store
            .list_backend_job_recording_child_ids(TEST_WORKSPACE_ID, &f.job_id, None, 100)
            .unwrap()
            .is_empty()
    );
    f.assert_saved_chunks(api);
    assert!(f.fixture.execution.take_inputs().is_empty());
}

#[tokio::test]
async fn finite_recording_reopens_multiple_pages_without_flooding_host_control() {
    let f = RecordingFixture::new(false).await;
    let api = &f.fixture.api;
    let input = api
        .config_store
        .get_backend_job_recording_input(TEST_WORKSPACE_ID, &f.job_id)
        .unwrap()
        .unwrap();
    let template = api
        .store
        .get_backend_job(TEST_WORKSPACE_ID, &f.job_id)
        .unwrap()
        .unwrap()
        .request;
    for index in 0..120 {
        let mut request = template.clone();
        request.job_id = format!("backlog:{index:03}");
        api.config_store
            .reserve_backend_job_recording_input(
                TEST_WORKSPACE_ID,
                &request,
                &input.scope,
                &f.chunks,
                &Utc::now().to_rfc3339(),
            )
            .unwrap();
    }
    f.fixture
        .store
        .begin_subject_deletion(&input.scope.subject_id, "backlog-target-deleted")
        .unwrap();
    let gate = api.backend_job_recovery.pause_steps_for_test().await;
    api.backend_job_recovery.resume_after_test_pause();
    backend_recording_job::recover(api).unwrap();
    backend_recording_job::recover(api).unwrap();
    let owner_count = api.backend_job_recovery.recovery_owner_ids().len();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        api.backend_job_recovery
            .recording_before_admission
            .notified(),
    )
    .await
    .unwrap();
    let control = api
        .host_executors
        .host()
        .unwrap()
        .run(|| Ok::<_, Error>("control admitted"))
        .await;
    drop(gate);
    let settled = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let changed = api.backend_job_recovery.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if api.backend_job_recovery.recovery_owner_ids().is_empty() {
                break;
            }
            changed.await;
        }
    })
    .await;
    api.stop_backend_job_recovery().await;
    assert_eq!(
        owner_count, 121,
        "one owner per saved input across pages, even after repeated recovery"
    );
    assert_eq!(control.unwrap(), "control admitted");
    settled.expect("all recording owners must finish without starving later pages");
    let reopened = f.reopened_api();
    assert!(
        reopened
            .config_store
            .list_active_backend_job_recording_ids(TEST_WORKSPACE_ID, None, 100)
            .unwrap()
            .is_empty()
    );
    for index in 0..120 {
        let id = format!("backlog:{index:03}");
        let job = reopened
            .store
            .get_backend_job(TEST_WORKSPACE_ID, &id)
            .unwrap()
            .unwrap();
        assert_eq!(job.state, BackendJobState::Completed);
        assert_eq!(
            job.result,
            Some(serde_json::json!({"application":"not_required"}))
        );
        assert!(
            reopened
                .config_store
                .list_backend_job_recording_child_ids(TEST_WORKSPACE_ID, &id, None, 100)
                .unwrap()
                .is_empty()
        );
    }
    f.assert_saved_chunks(&reopened);
    assert!(f.fixture.execution.take_inputs().is_empty());
}

#[tokio::test]
async fn finite_recording_deleted_subject_finishes_not_required_without_recreation() {
    erased_recording_target_is_not_required(true).await;
}

#[tokio::test]
async fn finite_recording_erased_session_finishes_not_required_without_restart() {
    erased_recording_target_is_not_required(false).await;
}

#[tokio::test]
async fn finite_recording_resumes_checkpoint_tail_without_source_and_waits_for_child_cleanup() {
    let f = RecordingFixture::new(true).await;
    let api = &f.fixture.api;
    let grant = f.fixture.request.grants.subjektiv_journal.as_ref().unwrap();
    release_saved_source(
        api,
        &f.fixture.store,
        &f.fixture.host,
        &f.fixture.source,
        &grant.subject_id,
    );
    assert!(
        api.store
            .get_worker_registry(TEST_WORKSPACE_ID, &f.fixture.source)
            .unwrap()
            .is_none()
    );
    assert!(!backend_recording_job::step(api, &f.job_id).unwrap());
    let first = f.child(api);
    let input = subjektiv::journal::recording::JournalInput::decode(&first.request.input).unwrap();
    assert_eq!(input.capture, f.chunks[0]);
    assert_eq!(
        first.request.source_worker.as_ref(),
        Some(&f.fixture.source)
    );
    assert!(!backend_recording_job::step(api, &f.job_id).unwrap());
    assert_eq!(f.child(api).request.job_id, first.request.job_id);
    let dispatched = f.dispatch_child(api);
    f.checkpoint(api, &dispatched);
    let journal = f
        .fixture
        .store
        .read_journal(&f.fixture.host, &f.fixture.id)
        .unwrap();
    assert_eq!((journal.process_order, journal.process_byte), (0, 8));
    let completed = api
        .store
        .get_backend_job_attempt(
            TEST_WORKSPACE_ID,
            &first.request.job_id,
            &dispatched.attempt.attempt_id,
        )
        .unwrap()
        .unwrap();
    assert_eq!(completed.state, BackendJobAttemptState::Completed);
    assert_eq!(
        completed.worker_cleanup_state,
        Some(BackendJobWorkerCleanupState::Pending)
    );
    assert!(!backend_recording_job::step(api, &f.job_id).unwrap());
    assert_eq!(f.child(api).request.job_id, first.request.job_id);
    assert_eq!(
        api.store
            .get_backend_job(TEST_WORKSPACE_ID, &f.job_id)
            .unwrap()
            .unwrap()
            .state,
        BackendJobState::Pending
    );
    f.assert_saved_chunks(api);
    // No background recovery: explicitly run the real cleanup, not a fabricated
    // cleanup receipt. Stop again before the Host's next deterministic step.
    api.backend_job_recovery
        .stopping
        .store(false, Ordering::Release);
    let cleanup = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        api.run_backend_job_cleanup(completed),
    )
    .await;
    api.stop_backend_job_recovery().await;
    cleanup.expect("first prefix cleanup timed out").unwrap();
    let reopened = f.reopened_api();
    assert!(!backend_recording_job::step(&reopened, &f.job_id).unwrap());
    let tail = f.child(&reopened);
    assert_ne!(tail.request.job_id, first.request.job_id);
    let input = subjektiv::journal::recording::JournalInput::decode(&tail.request.input).unwrap();
    assert_eq!(input.capture, f.chunks[1]);
    assert_eq!(input.selected_ranges[0].bytes.start, 8);
    assert_eq!(
        input.selected_ranges[0].bytes.end,
        f.fixture.range.bytes.end
    );
    assert!(!backend_recording_job::step(&reopened, &f.job_id).unwrap());
    assert_eq!(f.child(&reopened).request.job_id, tail.request.job_id);
    let dispatched = f.dispatch_child(&reopened);
    f.checkpoint(&reopened, &dispatched);
    let journal = open_subjektiv_store(&reopened)
        .unwrap()
        .read_journal(&f.fixture.host, &f.fixture.id)
        .unwrap();
    assert_eq!((journal.process_order, journal.process_byte), (1, 0));
    assert!(
        !backend_recording_job::step(&reopened, &f.job_id).unwrap(),
        "even a complete cursor cannot bypass pending child cleanup"
    );
    let completed = reopened
        .store
        .get_backend_job_attempt(
            TEST_WORKSPACE_ID,
            &tail.request.job_id,
            &dispatched.attempt.attempt_id,
        )
        .unwrap()
        .unwrap();
    reopened
        .backend_job_recovery
        .stopping
        .store(false, Ordering::Release);
    let cleanup = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        reopened.run_backend_job_cleanup(completed),
    )
    .await;
    reopened.stop_backend_job_recovery().await;
    cleanup.expect("tail cleanup timed out").unwrap();
    assert!(backend_recording_job::step(&reopened, &f.job_id).unwrap());
    let settled = reopened
        .store
        .get_backend_job(TEST_WORKSPACE_ID, &f.job_id)
        .unwrap()
        .unwrap();
    assert_eq!(settled.state, BackendJobState::Completed);
    assert_eq!(
        settled.result,
        Some(serde_json::json!({"application":"applied"}))
    );
    assert!(backend_recording_job::step(&reopened, &f.job_id).unwrap());
    assert_eq!(
        reopened
            .config_store
            .list_backend_job_recording_child_ids(TEST_WORKSPACE_ID, &f.job_id, None, 100,)
            .unwrap()
            .len(),
        2
    );
    f.assert_saved_chunks(&reopened);
    assert!(
        reopened
            .store
            .get_worker_registry(TEST_WORKSPACE_ID, &f.fixture.source)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        f.fixture.execution.take_inputs().len(),
        2,
        "one dispatch per saved prefix, no source restart"
    );
}
