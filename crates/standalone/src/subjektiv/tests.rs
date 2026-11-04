//! Consolidation recovery connects the production Subject receipt store, SQLite Job
//! store and local recovery dispatcher. Fixtures persist an interrupted attempt;
//! the named joined-execution contrast substitutes that execution boundary's
//! cleanup receipt, not the domain lookup. No test here proves an OS crash/drain.
use super::*;

struct ConsolidationRecoveryFixture {
    body: SubjectConnection,
    jobs: crate::jobs::StandaloneJobs,
    saved: crate::jobs::JobSnapshot,
    execution_owner: crate::job_store::JobStore,
    _temp: tempfile::TempDir,
}

impl Drop for ConsolidationRecoveryFixture {
    fn drop(&mut self) {
        // Revoke this fixture's Host before deleting its isolated storage. This
        // does not settle the retained prior execution or manufacture drain proof.
        self.body.host.live.store(false, Ordering::Release);
    }
}

impl ConsolidationRecoveryFixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let catalog = StandaloneSubjects::open(temp.path().join("state")).unwrap();
        let subject = catalog.create("recovery-owner", None).unwrap();
        let body = catalog
            .connect(&subject.id, protocol::WorkerId::now_v7())
            .unwrap();
        body.host
            .catalog
            .store
            .stage_candidate(subjektiv::SubjectStagingRecord::attach(
                &subject.id,
                memory::extract::StagingRecord::from_candidate(
                    "candidate",
                    "dedup",
                    memory::schema::SourceRef {
                        segment_id: "segment".into(),
                        range: [1, 1],
                    },
                    memory::extract::ExtractedCandidate {
                        kind: memory::extract::CandidateKind::Lesson,
                        claim: "claim".into(),
                        why_useful: "reason".into(),
                        staleness: None,
                        evidence_ids: vec![],
                    },
                    vec![],
                    vec![],
                ),
            ))
            .unwrap();
        let grant = LocalConsolidationGrant {
            binding: body.binding.clone(),
            candidate_ids: vec!["candidate".into()],
        };
        let request = job::JobRequest {
            job_id: "consolidation-recovery".into(),
            purpose: "subjektiv_consolidation".into(),
            input_ref: format!("subjektiv://{}/consolidation", subject.id),
            input: serde_json::json!({"subject_id":subject.id,"candidate_ids":grant.candidate_ids}),
            instruction: "Resolve the immutable batch".into(),
            profile: "builtin:standalone-subjektiv-consolidation".into(),
            serialization_key: Some(format!("subjektiv:{}", subject.id)),
            limits: Default::default(),
        };
        let mut store = crate::job_store::JobStore::open(&body.jobs_path).unwrap();
        let reserved = store
            .reserve_granted(request, Some(serde_json::to_value(grant).unwrap()))
            .unwrap();
        store
            .dispatch(&reserved.request.job_id, &reserved.attempt.attempt_id, 1)
            .unwrap();
        let saved = store
            .finish(
                &reserved.request.job_id,
                &reserved.attempt.attempt_id,
                crate::jobs::JobAttemptState::Failed,
                Some("host_restart_execution_unconfirmed: retained prior owner"),
            )
            .unwrap();
        let jobs =
            crate::jobs::StandaloneJobs::open(&body.jobs_path, temp.path().to_path_buf()).unwrap();
        body.host.bind_jobs(jobs.clone()).unwrap();
        Self {
            _temp: temp,
            body,
            jobs,
            saved,
            execution_owner: store,
        }
    }

    fn confirm_execution_joined(&mut self) {
        // The execution boundary is substituted in this domain-consumer fixture;
        // use its exact owner/run to commit the receipt through the real store.
        self.execution_owner
            .confirm_execution_cleanup(
                &self.saved.request.job_id,
                &self.saved.executions[0],
                "fixture-owned-execution-joined",
            )
            .unwrap();
        self.saved = self.jobs.get(&self.saved.request.job_id).unwrap();
    }

    fn resolve_candidate(&self) {
        self.body
            .host
            .catalog
            .store
            .decide_candidate(
                &self.body.binding.subject_id,
                subjektiv::CandidateDecisionRequest {
                    request_id: "decision".into(),
                    candidate_id: "candidate".into(),
                    reason: "invalid".into(),
                    decision: subjektiv::CandidateDecision::Close {
                        action: subjektiv::StagingResolutionAction::Invalid,
                        affected_memory: vec![],
                    },
                },
            )
            .unwrap();
    }

    fn generation(&self, attempt_id: &str) -> subjektiv::SurfaceGeneration {
        self.body
            .host
            .catalog
            .store
            .prepare_job_surface_generation(
                &self.body.binding.subject_id,
                Some((&self.saved.request.job_id, attempt_id)),
            )
            .unwrap()
    }

    fn publish(&self, generation: &subjektiv::SurfaceGeneration) {
        self.body
            .host
            .catalog
            .store
            .publish_surface_generation(&self.body.binding.subject_id, &generation.id, vec![])
            .unwrap();
    }
}

#[tokio::test]
async fn consolidation_recovery_accepts_same_attempt_receipts_without_claiming_owner_drain() {
    for availability in ["ready", "failed"] {
        let fixture = ConsolidationRecoveryFixture::new();
        fixture.resolve_candidate();
        let generation = fixture.generation(&fixture.saved.attempt.attempt_id);
        if availability == "ready" {
            fixture.publish(&generation);
        } else {
            fixture
                .body
                .host
                .catalog
                .store
                .fail_surface_generation(
                    &fixture.body.binding.subject_id,
                    &generation.id,
                    "model_failure",
                )
                .unwrap();
        }
        let resolution = serde_json::to_value(
            fixture
                .body
                .host
                .catalog
                .store
                .staging_resolution(&fixture.body.binding.subject_id, "candidate")
                .unwrap(),
        )
        .unwrap();
        assert!(fixture.jobs.pending().unwrap().is_empty());
        fixture.body.host.recover_jobs();
        let completed = fixture.jobs.get(&fixture.saved.request.job_id).unwrap();
        assert_eq!(completed.state, crate::jobs::JobState::Completed);
        assert_eq!(
            completed.attempt.attempt_id,
            fixture.saved.attempt.attempt_id
        );
        assert_eq!(completed.attempt.number, fixture.saved.attempt.number);
        assert_eq!(completed.input_digest, fixture.saved.input_digest);
        assert_eq!(completed.executions, fixture.saved.executions);
        assert!(!completed.executions_released());
        assert!(completed.acknowledged);
        let result = completed.result.as_ref().unwrap();
        assert_eq!(result["surface"]["generation_id"], generation.id);
        assert_eq!(result["surface"]["availability"], availability);
        assert_eq!(
            result["candidate_dispositions"],
            serde_json::json!([{"candidate_id":"candidate","action":"invalid"}])
        );
        if availability == "failed" {
            assert_eq!(result["surface"]["reason_code"], "model_failure");
        }
        fixture.body.host.recover_jobs();
        assert_eq!(
            fixture.jobs.get(&completed.request.job_id).unwrap(),
            completed
        );
        assert_eq!(
            serde_json::to_value(
                fixture
                    .body
                    .host
                    .catalog
                    .store
                    .staging_resolution(&fixture.body.binding.subject_id, "candidate")
                    .unwrap(),
            )
            .unwrap(),
            resolution
        );
        // A separate production store read proves settlement was persisted, not
        // merely projected by the dispatcher; no execution cleanup is inferred.
        let reopened = crate::job_store::JobStore::open(&fixture.body.jobs_path).unwrap();
        assert_eq!(reopened.get(&completed.request.job_id).unwrap(), completed);
    }
}

#[tokio::test]
async fn consolidation_recovery_reads_saved_result_after_execution_join_and_writer_fence() {
    let mut fixture = ConsolidationRecoveryFixture::new();
    fixture.resolve_candidate();
    let generation = fixture.generation(&fixture.saved.attempt.attempt_id);
    fixture.publish(&generation);
    fixture.confirm_execution_joined();
    assert!(fixture.saved.executions_released());
    assert!(fixture.saved.executions[0].writes_revoked);
    assert!(
        fixture
            .body
            .host
            .catalog
            .store
            .journal_writer_active(&fixture.body.host.writer)
            .unwrap()
    );

    fixture.body.host.recover_jobs();
    let completed = fixture.jobs.get(&fixture.saved.request.job_id).unwrap();
    assert_eq!(completed.state, crate::jobs::JobState::Completed);
    assert_eq!(
        completed.attempt.attempt_id,
        fixture.saved.attempt.attempt_id
    );
    assert_eq!(completed.input_digest, fixture.saved.input_digest);
    assert_eq!(completed.executions, fixture.saved.executions);
    assert!(completed.executions_released());
    assert_eq!(
        completed.result.as_ref().unwrap()["surface"]["generation_id"],
        generation.id
    );
}

#[tokio::test]
async fn consolidation_recovery_never_replays_incomplete_foreign_or_stale_receipts() {
    for case in ["missing_surface", "unresolved", "foreign_attempt", "stale"] {
        let fixture = ConsolidationRecoveryFixture::new();
        if case != "unresolved" {
            fixture.resolve_candidate();
        }
        if case != "missing_surface" {
            let attempt = if case == "foreign_attempt" {
                "other-attempt"
            } else {
                &fixture.saved.attempt.attempt_id
            };
            let generation = fixture.generation(attempt);
            fixture.publish(&generation);
        }
        if case == "stale" {
            fixture
                .body
                .host
                .catalog
                .store
                .create_memory(
                    &fixture.body.binding.subject_id,
                    subjektiv::MemoryDraft::active(
                        memory::extract::CandidateKind::Lesson,
                        "new lesson",
                        "new body",
                        "reason",
                        "change",
                    ),
                )
                .unwrap();
        }
        let before_surface = fixture
            .body
            .host
            .catalog
            .store
            .resident_surface(&fixture.body.binding.subject_id)
            .unwrap();
        fixture.body.host.recover_jobs();
        tokio::task::yield_now().await;
        assert_eq!(
            fixture.jobs.get(&fixture.saved.request.job_id).unwrap(),
            fixture.saved,
            "{case}"
        );
        assert_eq!(
            fixture
                .body
                .host
                .catalog
                .store
                .resident_surface(&fixture.body.binding.subject_id)
                .unwrap(),
            before_surface,
            "{case}"
        );
    }
}

#[tokio::test]
async fn consolidation_recovery_rejects_retired_input_version_stop_and_released_writer() {
    for case in [
        "erased",
        "invalidated",
        "version_stopped",
        "released_writer",
    ] {
        let fixture = ConsolidationRecoveryFixture::new();
        fixture.resolve_candidate();
        let generation = fixture.generation(&fixture.saved.attempt.attempt_id);
        fixture.publish(&generation);
        match case {
            "erased" | "invalidated" => {
                fixture
                    .jobs
                    .reclaim_input(
                        &fixture.saved.request.job_id,
                        &fixture.saved.input_digest,
                        if case == "erased" {
                            crate::jobs::JobInputRetirementReason::ExplicitErasure
                        } else {
                            crate::jobs::JobInputRetirementReason::LegacyInvalidation
                        },
                    )
                    .unwrap();
            }
            "version_stopped" => {
                // Seed the persisted cutover marker, not a new migration or an
                // execution termination proof.
                rusqlite::Connection::open(&fixture.body.jobs_path)
                    .unwrap()
                    .execute(
                        "INSERT INTO job_version_stops(job_id) VALUES (?1)",
                        [&fixture.saved.request.job_id],
                    )
                    .unwrap();
            }
            "released_writer" => {
                fixture
                    .body
                    .host
                    .catalog
                    .store
                    .release_journal_writer(&fixture.body.host.writer)
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let before = fixture.jobs.get(&fixture.saved.request.job_id).unwrap();
        assert!(
            fixture
                .body
                .host
                .recover_consolidation_job(&fixture.jobs, &fixture.saved)
                .is_err(),
            "{case}"
        );
        fixture.body.host.recover_jobs();
        assert_eq!(
            fixture.jobs.get(&fixture.saved.request.job_id).unwrap(),
            before,
            "{case}"
        );
        assert!(before.result.is_none());
        assert!(!before.executions_released());
    }
}

#[test]
fn consolidation_recovery_rejects_stale_attempt_and_digest_observations() {
    let fixture = ConsolidationRecoveryFixture::new();
    fixture.resolve_candidate();
    let generation = fixture.generation(&fixture.saved.attempt.attempt_id);
    fixture.publish(&generation);
    for change_attempt in [false, true] {
        let mut stale = fixture.saved.clone();
        if change_attempt {
            stale.attempt.attempt_id = "other-attempt".into();
        } else {
            stale.input_digest = "other-digest".into();
        }
        assert!(
            fixture
                .body
                .host
                .recover_consolidation_job(&fixture.jobs, &stale)
                .is_err()
        );
        assert_eq!(
            fixture.jobs.get(&fixture.saved.request.job_id).unwrap(),
            fixture.saved
        );
    }
}

#[test]
fn catalog_reopen_preserves_random_scope_subject_and_behavior() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("state");
    let catalog = StandaloneSubjects::open(&root).unwrap();
    let subject = catalog
        .create(
            "continuing-owner",
            Some("Keep user behavior separate from Memory."),
        )
        .unwrap();
    let scope = catalog.scope_id().to_owned();
    assert!(uuid::Uuid::parse_str(&scope).is_ok());
    assert!(uuid::Uuid::parse_str(subject.id.strip_prefix("subject-").unwrap()).is_ok());
    assert_ne!(scope, subject.id);
    drop(catalog);
    let reopened = StandaloneSubjects::open_existing(&root).unwrap();
    assert_eq!(reopened.scope_id(), scope);
    assert_eq!(reopened.list().unwrap(), vec![subject]);
}

#[test]
fn implicit_open_does_not_create_subject_storage() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("missing");
    assert!(StandaloneSubjects::open_existing(&root).is_err());
    assert!(!root.exists());
    assert!(StandaloneSubjects::open_existing(temp.path()).is_err());
    assert!(!temp.path().join("subjektiv").exists());
}

#[test]
fn subject_kernel_lease_is_not_worker_or_profile_scoped() {
    let temp = tempfile::tempdir().unwrap();
    let catalog = StandaloneSubjects::open(temp.path().join("state")).unwrap();
    let a = catalog.create("a", None).unwrap();
    let b = catalog.create("b", None).unwrap();
    let first_id = protocol::WorkerId::now_v7();
    let mut first = catalog.connect(&a.id, first_id).unwrap();
    assert!(matches!(
        StandaloneSubjects::open_existing(&temp.path().join("state"))
            .unwrap()
            .connect(&a.id, protocol::WorkerId::now_v7()),
        Err(SubjectError::Active(_))
    ));
    let mut second = StandaloneSubjects::open_existing(&temp.path().join("state"))
        .unwrap()
        .connect(&b.id, protocol::WorkerId::now_v7())
        .unwrap();
    first.close().unwrap();
    assert!(
        StandaloneSubjects::open_existing(&temp.path().join("state"))
            .unwrap()
            .connect(&a.id, protocol::WorkerId::now_v7())
            .is_err(),
        "kernel lease release is not durable body release"
    );
    let mut resumed = StandaloneSubjects::open_existing(&temp.path().join("state"))
        .unwrap()
        .connect(&a.id, first_id)
        .unwrap();
    resumed.close().unwrap();
    second.close().unwrap();
}

#[test]
fn closed_capabilities_and_foreign_worker_context_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let catalog = StandaloneSubjects::open(temp.path().join("state")).unwrap();
    let subject = catalog.create("owner", None).unwrap();
    let id = protocol::WorkerId::now_v7();
    let mut connection = catalog.connect(&subject.id, id).unwrap();
    let port = connection.host.clone();
    let foreign = SubjektivHostContext {
        worker_id: protocol::WorkerId::now_v7().to_string(),
        session_id: session_store::SessionId::now_v7().to_string(),
    };
    let read = SubjektivMemoryBackendOperation::ResidentContext(
        memory::backend::MemoryResidentSummaryOperation {},
    );
    assert!(port.memory(&foreign, read.clone()).is_err());
    let owner = SubjektivHostContext {
        worker_id: format!("standalone-{id}"),
        session_id: session_store::SessionId::now_v7().to_string(),
    };
    assert!(port.memory(&owner, read.clone()).is_ok());
    let local_operations = [
        "memory.resident.read",
        "memory.resident.supply",
        "memory.staging.write",
        "journal.startup.read",
        "journal.startup.supply",
        "journal.compaction.read",
        "journal.compaction.supply",
        "journal.recording.request",
    ];
    for operation in local_operations {
        assert!(
            port.authorize_operation(&owner, operation).is_ok(),
            "{operation}"
        );
        assert!(
            port.authorize_operation(&foreign, operation).is_err(),
            "{operation}"
        );
    }
    for operation in [
        "chat.read",
        "chat.send",
        "worker.messaging",
        "memory.candidate.decide",
        "memory.surface.publish",
        "memory.staging.list",
        "journal",
        "journal.startup",
        "journal.startup.read.future",
    ] {
        assert!(
            matches!(
                port.authorize_operation(&owner, operation),
                Err(SubjektivHostError::PermissionDenied(_))
            ),
            "local body must not grant {operation}"
        );
    }
    assert!(
        port.record_session(&owner, false).is_err(),
        "unattributed Session is not read authority"
    );
    connection.close().unwrap();
    for operation in local_operations {
        assert!(
            port.authorize_operation(&owner, operation).is_err(),
            "released capability: {operation}"
        );
    }
    assert!(port.memory(&owner, read).is_err());
}

#[cfg(unix)]
#[test]
fn scope_rejects_symlinks_and_non_private_roots() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = tempfile::tempdir().unwrap();
    let alias = temp.path().join("alias");
    let real = temp.path().join("real");
    fs::create_dir(&real).unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
    symlink(&real, &alias).unwrap();
    assert!(StandaloneSubjects::open(&alias).is_err());
    let public = temp.path().join("public");
    fs::create_dir(&public).unwrap();
    fs::set_permissions(&public, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(StandaloneSubjects::open(&public).is_err());
    let catalog = StandaloneSubjects::open(&real).unwrap();
    drop(catalog);
    let scope = real.join("subjektiv/scope.json");
    let original = fs::read(&scope).unwrap();
    fs::remove_file(&scope).unwrap();
    let foreign = temp.path().join("foreign.json");
    fs::write(&foreign, &original).unwrap();
    symlink(&foreign, &scope).unwrap();
    assert!(StandaloneSubjects::open_existing(&real).is_err());
    assert_eq!(fs::read(&foreign).unwrap(), original);
}

#[test]
fn different_local_roots_do_not_authorize_shared_subject_ids() {
    let temp = tempfile::tempdir().unwrap();
    let first = StandaloneSubjects::open(temp.path().join("first")).unwrap();
    let subject = first.create("owner", None).unwrap();
    let other = StandaloneSubjects::open(temp.path().join("other")).unwrap();
    assert_ne!(first.scope_id(), other.scope_id());
    assert!(
        other
            .connect(&subject.id, protocol::WorkerId::now_v7())
            .is_err()
    );
}

#[test]
fn consolidation_operation_policy_is_attempt_bound_and_does_not_inherit_body_rights() {
    struct Fence(AtomicBool, AtomicBool);
    impl crate::jobs::JobAttemptFence for Fence {
        fn enter(&self) -> Result<Box<dyn crate::jobs::JobEffectGuard + '_>, String> {
            if !self.1.load(Ordering::Acquire) {
                return Err("attempt admission closed".into());
            }
            self.ensure_live()?;
            Ok(Box::new(()))
        }

        fn ensure_live(&self) -> Result<(), String> {
            self.0
                .load(Ordering::Acquire)
                .then_some(())
                .ok_or("attempt revoked".into())
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let catalog = StandaloneSubjects::open(temp.path().join("state")).unwrap();
    let subject = catalog.create("owner", None).unwrap();
    let mut body = catalog
        .connect(&subject.id, protocol::WorkerId::now_v7())
        .unwrap();
    let fence = Arc::new(Fence(AtomicBool::new(true), AtomicBool::new(true)));
    let job = LocalConsolidationHost {
        host: body.host.clone(),
        grant: LocalConsolidationGrant {
            binding: body.binding.clone(),
            candidate_ids: vec![],
        },
        job_id: "job-1".into(),
        attempt_id: "attempt-1".into(),
        fence: fence.clone(),
    };
    let context = SubjektivHostContext {
        worker_id: "job:attempt-1".into(),
        session_id: "job:attempt-1".into(),
    };
    assert!(
        job.authorize_operation(&context, "memory.staging.list")
            .is_ok()
    );
    assert!(
        job.authorize_operation(&context, "memory.surface.supply")
            .is_ok()
    );
    let foreign = SubjektivHostContext {
        worker_id: "job:old-attempt".into(),
        session_id: "job:old-attempt".into(),
    };
    assert!(
        job.authorize_operation(&foreign, "memory.staging.list")
            .is_err()
    );
    for op in [
        "session.record",
        "memory.resident.read",
        "memory.resident.supply",
        "memory.extract.read",
        "memory.staging.write",
        "journal.startup.read",
        "chat.send",
        "memory",
    ] {
        assert!(
            matches!(
                job.authorize_operation(&context, op),
                Err(SubjektivHostError::PermissionDenied(_))
            ),
            "Job must not gain {op}"
        );
    }
    assert!(
        job.memory(
            &context,
            SubjektivMemoryBackendOperation::ReadCandidate(SubjektivMemoryCandidateReadRequest {
                candidate_id: "outside-batch".into()
            })
        )
        .is_err()
    );
    fence.1.store(false, Ordering::Release);
    assert!(
        job.authorize_operation(&context, "memory.staging.read")
            .is_ok()
    );
    assert!(matches!(
        job.memory(
            &context,
            SubjektivMemoryBackendOperation::ReadCandidate(SubjektivMemoryCandidateReadRequest {
                candidate_id: "outside-batch".into()
            })
        ),
        Err(SubjektivHostError::Unavailable(reason)) if reason == "attempt admission closed"
    ));
    fence.1.store(true, Ordering::Release);
    fence.0.store(false, Ordering::Release);
    assert!(
        job.authorize_operation(&context, "memory.staging.list")
            .is_err()
    );
    fence.0.store(true, Ordering::Release);
    body.close().unwrap();
    assert!(
        job.authorize_operation(&context, "memory.staging.list")
            .is_err()
    );
}

#[test]
fn session_registration_uses_real_capability_and_attribution_decisions() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("state");
    let catalog = StandaloneSubjects::open(&root).unwrap();
    let subject = catalog.create("registration-owner", None).unwrap();
    let id = protocol::WorkerId::now_v7();
    let mut connection = catalog.connect(&subject.id, id).unwrap();
    let port = connection.host.clone();
    let context = SubjektivHostContext {
        worker_id: format!("standalone-{id}"),
        session_id: session_store::SessionId::now_v7().to_string(),
    };
    assert_eq!(
        port.record_session(&context, false)
            .unwrap_err()
            .session_registration_cause(),
        SessionRegistrationCause::attribution_missing(),
    );
    let foreign = SubjektivHostContext {
        worker_id: "unbound-private-worker".into(),
        ..context.clone()
    };
    assert_eq!(
        port.record_session(&foreign, true)
            .unwrap_err()
            .session_registration_cause(),
        SessionRegistrationCause::binding_mismatch(),
    );
    // Normal factory startup creates the Worker Session store before registration.
    // Keep that real filesystem prerequisite without pre-creating attribution.
    let _sessions = session_store::FsStore::new_with_retention(
        root.join(id.to_string()).join("sessions"),
        crate::store::session_retention(&root),
    )
    .unwrap();
    let success = port.record_session(&context, true).unwrap();
    assert_eq!(success.subject_id, subject.id);
    assert_eq!(success.session_id, context.session_id);
    assert!(port.record_session(&context, false).is_ok());
    let reopened = StandaloneSubjects::open_existing(&root).unwrap();
    let actual = reopened
        .store
        .session_attribution(&context.session_id)
        .unwrap()
        .unwrap();
    assert_eq!(actual.subject_id, subject.id);
    assert_eq!(actual.worker_id, id.to_string());
    connection.close().unwrap();
    assert_eq!(
        port.record_session(&context, true)
            .unwrap_err()
            .session_registration_cause(),
        SessionRegistrationCause::denied(SessionPrincipal::Subject),
    );
}

#[derive(Clone)]
struct RegistrationUnusedModel;
#[async_trait::async_trait]
impl agen::llm_client::LlmClient for RegistrationUnusedModel {
    fn clone_boxed(&self) -> Box<dyn agen::llm_client::LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        _: agen::llm_client::Request,
    ) -> Result<agen::llm_client::client::ResponseStream, agen::llm_client::ClientError> {
        panic!("Session registration must not invoke a model")
    }
}

#[tokio::test]
async fn local_writer_revocation_during_registration_retains_known_cause_and_real_journal_source() {
    use session_store::{CombinedStore, FsStore, FsWorkerStore, WorkerMetadataStore};
    use worker::{
        WorkerBootstrap, WorkerBootstrapLayout, WorkerFilesystemAuthority, WorkerWorkspaceContext,
    };
    // The direct non-Browser caller uses the real bootstrap, local capability and
    // transactional Journal store. Only the unused model and revocation timing are substituted.
    for existing in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("state");
        let catalog = StandaloneSubjects::open(&root).unwrap();
        let subject = catalog.create("registration-race", None).unwrap();
        let id = protocol::WorkerId::now_v7();
        let mut connection = catalog.connect(&subject.id, id).unwrap();
        let port = connection.host.clone();
        let name = format!("standalone-{id}");
        let sessions = FsStore::new_with_retention(
            root.join(id.to_string()).join("sessions"),
            crate::store::session_retention(&root),
        )
        .unwrap();
        let metadata_root = temp.path().join("metadata");
        let metadata = FsWorkerStore::new(&metadata_root).unwrap();
        let mut manifest = manifest::WorkerManifest::from_toml(&format!("[worker]\nname='{name}'\n[model]\nscheme='anthropic'\nmodel_id='unused'\n[engine]\n[scope]\nallow=[]")).unwrap();
        manifest.feature.subjektiv.profile.enabled = true;
        manifest.feature.subjektiv.profile.extraction.enabled = false;
        let mut prepared = WorkerBootstrap::new(
            manifest,
            CombinedStore::new(sessions, metadata.clone()),
            worker::PromptCatalogSource::builtins_only(),
            WorkerWorkspaceContext::no_workspace(),
            WorkerFilesystemAuthority::None,
            WorkerBootstrapLayout::Direct {
                runtime_base: temp.path().join("runtime"),
            },
        )
        .with_model_client(RegistrationUnusedModel)
        .prepare()
        .await
        .unwrap();
        let worker = prepared.worker_mut();
        worker.enable_worker_metadata_write_through().unwrap();
        worker.bind_subjektiv_host(Some(port.clone()));
        let session = worker.session_id();
        let context = SubjektivHostContext {
            worker_id: name.clone(),
            session_id: session.to_string(),
        };
        if existing {
            port.record_session(&context, true).unwrap();
        }
        let revoke_store = port.catalog.store.clone();
        let writer = port.writer.clone();
        *port.registration_before_attribution.lock().unwrap() = Some(Box::new(move || {
            // This actual release follows the last preflight, before transactional bind.
            revoke_store.release_journal_writer(&writer).unwrap();
        }));
        let error = worker
            .finalize_subjektiv_session_attribution_retaining_startup_failure(
                true,
                worker::SubjektivSessionAttributionLifecycle::NewSession,
            )
            .unwrap_err();
        assert_eq!(
            error.session_registration_cause(),
            Some(SessionRegistrationCause::binding_mismatch())
        );
        let mut source = Some(&error as &(dyn std::error::Error + 'static));
        let mut journal_source = false;
        while let Some(current) = source {
            if let Some(journal) = current.downcast_ref::<subjektiv::journal::JournalError>() {
                assert!(matches!(
                    journal,
                    subjektiv::journal::JournalError::PermissionDenied(_)
                ));
                journal_source = true;
            }
            source = current.source();
        }
        assert!(
            journal_source,
            "original store refusal must survive the Worker wrapper"
        );
        assert_eq!(
            error.startup_metadata_save(),
            Some(protocol::startup_failure::StartupPersistenceOutcome::Persisted)
        );
        drop(prepared);
        drop(metadata);
        let reopened = FsWorkerStore::new(metadata_root)
            .unwrap()
            .read_by_name(&name)
            .unwrap()
            .unwrap();
        let session_store::worker_metadata::StartupFailureRead::Present(
            error_contract::DecodeFailure::Known(cause),
        ) = reopened.decode_startup_failure::<SessionRegistrationCause>()
        else {
            panic!("typed cause must really reopen")
        };
        assert_eq!(
            *cause.facts().cause(),
            SessionRegistrationCause::binding_mismatch()
        );
        assert!(
            port.catalog
                .store
                .session_attribution(&session.to_string())
                .unwrap()
                .is_some(),
            "committed attribution is independent from denied writer binding"
        );
        assert!(
            !port
                .catalog
                .store
                .journal_writer_active(&port.writer)
                .unwrap()
        );
        connection.close().unwrap();
    }
}
