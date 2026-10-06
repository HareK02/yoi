use super::*;
use crate::subjektiv::{
    CandidateDecision, CandidateDecisionRequest, MemoryDraft, MemoryRevisionTarget, SubjectRole,
    SubjectStagingRecord, SurfaceAvailability,
};
use memory::extract::{CandidateKind, ExtractedCandidate, StagingEvidence, StagingRecord};
use memory::schema::SourceRef;
use memory::schema::{EvidenceKind, EvidenceOrigin, EvidenceOriginKind};

fn stage(store: &crate::subjektiv::SubjektivStore, subject: &str, id: &str) {
    let origin = EvidenceOrigin {
        kind: EvidenceOriginKind::HumanInput,
        account_id: Some("test-human".into()),
        workspace_id: Some(TEST_WORKSPACE_ID.into()),
        runtime_id: None,
        worker_id: None,
        flow_selector: None,
        flow_definition_id: None,
        flow_definition_revision: None,
    };
    let record = StagingRecord::from_candidate(
        id,
        "test-extract",
        SourceRef {
            segment_id: "segment-1".into(),
            range: [1, 2],
        },
        ExtractedCandidate {
            kind: CandidateKind::Constraint,
            claim: "Keep receipts and confirmed memory".into(),
            why_useful: "No repeated application".into(),
            staleness: None,
            evidence_ids: vec!["e1".into()],
        },
        vec![StagingEvidence {
            id: "e1".into(),
            kind: EvidenceKind::new(EvidenceKind::MESSAGE),
            entry_range: Some([1, 2]),
            origin: Some(origin),
            excerpt: Some("Keep confirmed memory".into()),
            summary: None,
        }],
        vec![],
    );
    store
        .stage_candidate(SubjectStagingRecord::attach(subject, record))
        .unwrap();
}
fn context(worker: &RuntimeWorkerRef) -> server_api::ServerRequestContext {
    server_api::ServerRequestContext {
        actor: None,
        worker_source: None,
        runtime_source: Some(server_api::ServerRuntimeSource {
            runtime_id: worker.runtime_id.clone(),
            worker_id: Some(worker.worker_id.clone()),
        }),
        origin: None,
        transport_headers: vec![],
    }
}
async fn op(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    operation: server_api::SubjektivMemoryBackendOperation,
) -> ApiResult<Json<server_api::SubjektivMemoryBackendResponse>> {
    scoped_subjektiv_memory_backend(
        State(api.clone()),
        AxumPath(ScopedWorkspacePath {
            workspace_id: TEST_WORKSPACE_ID.into(),
        }),
        context(worker),
        Json(server_api::SubjektivMemoryBackendRequest { operation }),
    )
    .await
}
fn resource(api: &WorkspaceApi, subject: &str) -> crate::backend_job::BackendJobRecord {
    api.store
        .find_active_backend_job_for_resource(
            TEST_WORKSPACE_ID,
            &crate::backend_job::subjektiv_serialization_key(subject),
        )
        .unwrap()
        .unwrap()
}
fn current_attempt(
    api: &WorkspaceApi,
    job: &crate::backend_job::BackendJobRecord,
) -> BackendJobAttemptRecord {
    api.store
        .get_backend_job_attempt(
            TEST_WORKSPACE_ID,
            &job.request.job_id,
            &crate::backend_job::attempt_id(&job.request.job_id, job.current_attempt),
        )
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn subjektiv_job_bounds_batch_auth_partial_receipts_surface_and_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(dir.path()).await;
    let store = open_subjektiv_store(&api).unwrap();
    let subject = store
        .create_subject(SubjectRole::new("test-subject").unwrap())
        .unwrap();
    stage(&store, &subject.id, "candidate-first");
    let started = start_subjektiv_staging_consolidation(
        api.clone(),
        &subject.id,
        MemoryConsolidateStagingOperation { force: true },
    )
    .await
    .unwrap();
    assert_eq!(started.status, "started");
    let job = resource(&api, &subject.id);
    assert_eq!(
        job.request.profile,
        "builtin:subjektiv-memory-consolidation"
    );
    let attempt = current_attempt(&api, &job);
    let worker = attempt.worker.as_ref().unwrap();
    assert_eq!(execution.take_inputs().len(), 1);
    assert!(
        api.store
            .require_current_worker_singleton_owner(TEST_WORKSPACE_ID, worker)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        subjektiv_subject_scope(&api, TEST_WORKSPACE_ID, &context(worker))
            .unwrap()
            .0,
        subject.id
    );
    assert!(subjektiv_subject_scope(&api, "foreign", &context(worker)).is_err());
    let ordinary = spawn_ticket_check_source(&api, "not-a-consolidator");
    assert!(subjektiv_subject_scope(&api, TEST_WORKSPACE_ID, &context(&ordinary)).is_err());
    // A premature result cannot generate a surface or complete unresolved work.
    assert!(
        op(
            &api,
            worker,
            server_api::SubjektivMemoryBackendOperation::PrepareSurface(
                server_api::SubjektivSurfacePrepareRequest {}
            )
        )
        .await
        .is_err()
    );
    stage(&store, &subject.id, "candidate-arrived-later");
    let again = start_subjektiv_staging_consolidation(
        api.clone(),
        &subject.id,
        MemoryConsolidateStagingOperation { force: true },
    )
    .await
    .unwrap();
    assert_eq!(again.status, "skipped_existing_job");
    assert!(execution.take_inputs().is_empty());
    assert_eq!(resource(&api, &subject.id).request, job.request);
    assert!(
        subjektiv_job::require_candidate_grant(
            &api,
            TEST_WORKSPACE_ID,
            worker,
            "candidate-arrived-later"
        )
        .is_err()
    );
    let decision = CandidateDecisionRequest {
        request_id: "exact-apply".into(),
        candidate_id: "candidate-first".into(),
        reason: "retain the learned constraint".into(),
        decision: CandidateDecision::Apply {
            target: MemoryRevisionTarget::Create,
            draft: MemoryDraft::active(
                CandidateKind::Constraint,
                "Keep committed memory",
                "Applicable across failures",
                "Prevents duplicate application",
                "confirmed from candidate",
            ),
        },
    };
    let receipt = store
        .decide_candidate(&subject.id, decision.clone())
        .unwrap();
    let replay = store.decide_candidate(&subject.id, decision).unwrap();
    assert_eq!(
        receipt.memory.as_ref().unwrap().id,
        replay.memory.as_ref().unwrap().id
    );
    assert_eq!(receipt.store_revision, replay.store_revision);
    let generation = store
        .prepare_job_surface_generation(
            &subject.id,
            Some((&job.request.job_id, &attempt.attempt_id)),
        )
        .unwrap();
    let Json(failed) = op(
        &api,
        worker,
        server_api::SubjektivMemoryBackendOperation::FailSurface(
            server_api::SubjektivSurfaceFailureRequest {
                generation_id: generation.id.clone(),
                reason_code: "editor_failed".into(),
            },
        ),
    )
    .await
    .unwrap();
    assert!(
        matches!(failed, server_api::SubjektivMemoryBackendResponse::SurfaceFailed(output) if output.status == "failed_confirmed" && output.store_revision == generation.store_revision)
    );
    let result = serde_json::json!({"subject_id": subject.id, "candidate_ids": ["candidate-first"], "surface": {"availability":"failed", "generation_id":generation.id, "store_revision":generation.store_revision,"reason_code":"editor_failed"}});
    let submission = BackendJobResultSubmission {
        job_id: job.request.job_id.clone(),
        attempt_id: attempt.attempt_id.clone(),
        input_revision: job.request.input_revision.clone(),
        result,
    };
    assert!(
        api.accept_backend_job_result(&ordinary, &submission)
            .is_err()
    );
    let mut forged = submission.clone();
    forged.result["surface"]["reason_code"] = serde_json::json!("not_recorded");
    assert!(api.accept_backend_job_result(worker, &forged).is_err());
    seed_worker_session_for_cleanup(dir.path(), worker);
    let accepted = api.accept_backend_job_result(worker, &submission).unwrap();
    assert_eq!(accepted.job.state, BackendJobState::Completed);
    assert_eq!(
        store.resident_surface(&subject.id).unwrap().availability,
        SurfaceAvailability::Failed
    );
    assert_eq!(store.pending_staging_backlog(&subject.id).unwrap().0, 1);
    assert!(
        store
            .memory(&subject.id, &receipt.memory.unwrap().id)
            .unwrap()
            .is_some()
    );
    assert!(subjektiv_subject_scope(&api, TEST_WORKSPACE_ID, &context(worker)).is_err());
    wait_for_backend_job_worker_cleanup(
        &api,
        &job.request.job_id,
        &attempt.attempt_id,
        BackendJobWorkerCleanupState::Completed,
    )
    .await;
    assert!(
        api.store
            .get_backend_job(TEST_WORKSPACE_ID, &job.request.job_id)
            .unwrap()
            .unwrap()
            .result
            .is_some()
    );
    let next = start_subjektiv_staging_consolidation(
        api.clone(),
        &subject.id,
        MemoryConsolidateStagingOperation { force: true },
    )
    .await
    .unwrap();
    assert_eq!(next.status, "started");
    assert_eq!(
        resource(&api, &subject.id)
            .request
            .grants
            .subjektiv_consolidation
            .unwrap()
            .candidate_ids,
        ["candidate-arrived-later"]
    );
}

#[tokio::test]
async fn subjektiv_job_surface_only_requires_attempt_owned_generation_and_real_outcome() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _) = test_api_with_recording_backend(dir.path()).await;
    let store = open_subjektiv_store(&api).unwrap();
    let subject = store
        .create_subject(SubjectRole::new("surface-only").unwrap())
        .unwrap();
    let old = store.prepare_surface_generation(&subject.id).unwrap();
    assert_eq!(
        start_subjektiv_staging_consolidation(
            api.clone(),
            &subject.id,
            MemoryConsolidateStagingOperation { force: false }
        )
        .await
        .unwrap()
        .status,
        "started"
    );
    let job = resource(&api, &subject.id);
    let attempt = current_attempt(&api, &job);
    let worker = attempt.worker.as_ref().unwrap();
    assert!(
        store
            .require_job_surface_generation(
                &subject.id,
                &old.id,
                &job.request.job_id,
                &attempt.attempt_id
            )
            .is_err()
    );
    let generation = store
        .prepare_job_surface_generation(
            &subject.id,
            Some((&job.request.job_id, &attempt.attempt_id)),
        )
        .unwrap();
    assert!(
        store
            .require_job_surface_generation(
                &subject.id,
                &generation.id,
                &job.request.job_id,
                "wrong-attempt"
            )
            .is_err()
    );
    let snapshot = store
        .publish_surface_generation(&subject.id, &generation.id, vec![])
        .unwrap();
    assert!(
        op(
            &api,
            worker,
            server_api::SubjektivMemoryBackendOperation::FailSurface(
                server_api::SubjektivSurfaceFailureRequest {
                    generation_id: generation.id.clone(),
                    reason_code: "editor_failed".into()
                }
            )
        )
        .await
        .is_err(),
        "ready protection is not a confirmed failed outcome"
    );
    let Json(prepared) = op(
        &api,
        worker,
        server_api::SubjektivMemoryBackendOperation::PrepareSurface(
            server_api::SubjektivSurfacePrepareRequest {},
        ),
    )
    .await
    .unwrap();
    assert!(
        matches!(prepared, server_api::SubjektivMemoryBackendResponse::SurfacePrepared(output) if output.current_snapshot_id.as_deref() == Some(snapshot.id.as_str()))
    );
    seed_worker_session_for_cleanup(dir.path(), worker);
    let submission = BackendJobResultSubmission {
        job_id: job.request.job_id.clone(),
        attempt_id: attempt.attempt_id.clone(),
        input_revision: job.request.input_revision.clone(),
        result: serde_json::json!({"subject_id":subject.id,"candidate_ids":[],"surface":{"availability":"ready","generation_id":generation.id,"store_revision":generation.store_revision,"snapshot_id":snapshot.id}}),
    };
    api.accept_backend_job_result(worker, &submission).unwrap();
    wait_for_backend_job_worker_cleanup(
        &api,
        &job.request.job_id,
        &attempt.attempt_id,
        BackendJobWorkerCleanupState::Completed,
    )
    .await;
    assert_eq!(
        start_subjektiv_staging_consolidation(
            api.clone(),
            &subject.id,
            MemoryConsolidateStagingOperation { force: true }
        )
        .await
        .unwrap()
        .status,
        "skipped_empty"
    );
    assert!(
        api.accept_backend_job_result(worker, &submission)
            .unwrap()
            .replayed
    );
}

#[tokio::test]
async fn backend_job_profile_resolution_rejects_ungranted_requirements_without_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(dir.path()).await;
    let mut request = BackendJobRequest {
        job_id: "profile-resolution".into(),
        purpose: "any-purpose".into(),
        input_revision: "1".into(),
        input_ref: "test://profiles".into(),
        input: serde_json::json!({}),
        instruction: "return structured result".into(),
        profile: "builtin:backend-job".into(),
        grants: Default::default(),
        serialization_key: None,
        source_worker: None,
        notification_target: None,
        limits: Default::default(),
    };
    let (selector, bundle) = api.resolve_backend_job_profile(&request).unwrap();
    assert_eq!(selector, ProfileSelector::Builtin(request.profile.clone()));
    assert_eq!(
        crate::profile_settings::resolve_profile_manifest_from_config_bundle(
            &bundle,
            &request.profile
        )
        .unwrap()
        .engine
        .instruction,
        "internal.backend_job_system"
    );
    for profile in [
        "path:/etc/profile",
        "builtin:missing",
        "project:missing",
        "builtin:coder",
        "builtin:companion",
        "builtin:subjektiv-memory-consolidation",
    ] {
        request.profile = profile.into();
        assert!(api.dispatch_backend_job(&request).is_err(), "{profile}");
        assert!(
            api.store
                .get_backend_job(TEST_WORKSPACE_ID, &request.job_id)
                .unwrap()
                .is_none()
        );
    }
    assert!(execution.take_inputs().is_empty());
}

#[tokio::test]
async fn backend_job_project_recipe_is_resolved_and_delivered_without_system_override() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _) = test_api_with_recording_backend(dir.path()).await;
    let state = api
        .config_store
        .load_workspace_config(TEST_WORKSPACE_ID)
        .unwrap()
        .unwrap();
    let entrypoint = state.contract.entrypoints.first().unwrap();
    let root = state.snapshot.entries.get(entrypoint).unwrap();
    let config = config_commit_request_from_api(server_api::ConfigCommitRequest {
        base_revision: state.snapshot.revision, base_digest: state.snapshot.digest.clone(),
        entrypoints: state.contract.entrypoints.iter().map(|p| p.as_str().into()).collect(),
        changes: vec![server_api::ConfigTreeChange::Update {
            path: entrypoint.as_str().into(), expected_digest: root.content_digest.clone(),
            content: "{ profile = { entries = [{ selector = \"project:job-recipe\"; source = \"profiles/job-recipe.dcdl\"; }]; }; }".into(),
        }, server_api::ConfigTreeChange::Create {
            path: "profiles/job-recipe.dcdl".into(), content_type: server_api::ConfigContentType::Decodal,
            content: "{ slug = \"job-recipe\"; scope = \"workspace_read\"; model = { id = \"fixture-model\"; }; engine = { instruction = \"role.default\"; }; feature = { task = { enabled = true; }; }; }".into(),
        }],
    }).unwrap();
    commit_workspace_config_tree(&api, TEST_WORKSPACE_ID, &config).unwrap();
    let request = BackendJobRequest {
        job_id: "project-recipe-job".into(),
        purpose: "no-profile-switch".into(),
        input_revision: "r1".into(),
        input_ref: "test://recipe".into(),
        input: serde_json::json!({}),
        instruction: "Submit the result".into(),
        profile: "project:job-recipe".into(),
        grants: Default::default(),
        serialization_key: None,
        source_worker: None,
        notification_target: None,
        limits: Default::default(),
    };
    let (selector, bundle) = api.resolve_backend_job_profile(&request).unwrap();
    assert_eq!(
        selector,
        ProfileSelector::Named("project:job-recipe".into())
    );
    let manifest = crate::profile_settings::resolve_profile_manifest_from_config_bundle(
        &bundle,
        &request.profile,
    )
    .unwrap();
    assert_eq!(manifest.engine.instruction, "role.default");
    assert!(manifest.feature.task.enabled);
    let dispatched = api.dispatch_backend_job(&request).unwrap();
    let worker = dispatched.attempt.worker.unwrap();
    assert_eq!(
        api.runtime.worker(&worker).unwrap().profile.as_deref(),
        Some("project:job-recipe")
    );
}

#[tokio::test]
async fn subjektiv_job_legacy_cutover_waits_foreground_then_removes_only_owned_consolidator() {
    let dir = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(dir.path()).await;
    let store = open_subjektiv_store(&api).unwrap();
    let subject = store
        .create_subject(SubjectRole::new("legacy-cutover").unwrap())
        .unwrap();
    let subject_body = spawn_ticket_check_source(&api, "subject-body-protected");
    let key = format!("subjektiv-consolidation:{}", subject.id);
    let legacy = api
        .spawn_workspace_worker(
            EMBEDDED_WORKER_RUNTIME_ID,
            WorkerSpawnRequest {
                requested_worker_name: Some("not-authority-display".into()),
                singleton_key: Some(key.clone()),
                intent: WorkerSpawnIntent::WorkspaceOrchestrator,
                acceptance: WorkerSpawnAcceptanceRequirement::RunAccepted {
                    expected_segments: 0,
                },
                profile: ProfileSelector::Builtin(SUBJEKTIV_MEMORY_CONSOLIDATION_PROFILE.into()),
                ticket_assignment: None,
                initial_submit: vec![],
                workdir_attachment_requests: vec![],
                resolved_workdir_attachment_requests: vec![],
                resolved_workdir_attachments: vec![],
                resolved_config_bundle: None,
                resolved_workspace_api: None,
                resolved_memory_settings: None,
                resolved_subjektiv_attached: false,
                resolved_worker_observation_enabled: false,
                resolved_worker_observation_grants: vec![],
                resolved_control_operation: None,
            },
        )
        .unwrap()
        .worker
        .unwrap()
        .worker;
    seed_worker_session_for_cleanup(dir.path(), &legacy);
    execution.publish_worker_state(
        &legacy,
        protocol::WorkerStateSnapshot::from(protocol::WorkerStatus::Running),
    );
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while api
            .runtime
            .worker(&legacy)
            .unwrap()
            .worker_state
            .as_ref()
            .is_none_or(|s| s.state == protocol::WorkerState::Idle)
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        start_subjektiv_staging_consolidation(
            api.clone(),
            &subject.id,
            MemoryConsolidateStagingOperation { force: true }
        )
        .await
        .unwrap()
        .status,
        "skipped_legacy_inflight"
    );
    assert!(
        api.store
            .find_active_backend_job_for_resource(TEST_WORKSPACE_ID, &key)
            .unwrap()
            .is_none()
    );
    assert!(api.runtime.worker(&legacy).is_ok());
    execution.publish_worker_state(&legacy, protocol::WorkerStateSnapshot::initial());
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while api
            .runtime
            .worker(&legacy)
            .unwrap()
            .worker_state
            .as_ref()
            .is_none_or(|s| s.state != protocol::WorkerState::Idle)
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        start_subjektiv_staging_consolidation(
            api.clone(),
            &subject.id,
            MemoryConsolidateStagingOperation { force: true }
        )
        .await
        .unwrap()
        .status,
        "started"
    );
    assert!(api.runtime.worker(&legacy).is_err());
    assert!(
        api.store
            .current_worker_singleton_owner(TEST_WORKSPACE_ID, &key)
            .unwrap()
            .is_none()
    );
    assert!(api.runtime.worker(&subject_body).is_ok());
    assert!(store.subject(&subject.id).unwrap().is_some());
}

#[tokio::test]
async fn subjektiv_job_long_receipts_and_escaped_ids_fit_result_budget_without_losing_followup() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _) = test_api_with_recording_backend(dir.path()).await;
    let store = open_subjektiv_store(&api).unwrap();
    let subject = store
        .create_subject(SubjectRole::new("result-budget").unwrap())
        .unwrap();
    for i in 0..100 {
        stage(
            &store,
            &subject.id,
            &format!("candidate-{i:03}-{}", "\\\"".repeat(60)),
        );
    }
    start_subjektiv_staging_consolidation(
        api.clone(),
        &subject.id,
        MemoryConsolidateStagingOperation { force: true },
    )
    .await
    .unwrap();
    let job = resource(&api, &subject.id);
    let ids = job
        .request
        .grants
        .subjektiv_consolidation
        .as_ref()
        .unwrap()
        .candidate_ids
        .clone();
    assert!(!ids.is_empty() && ids.len() < 100);
    let attempt = current_attempt(&api, &job);
    let worker = attempt.worker.as_ref().unwrap();
    for id in &ids {
        store
            .decide_candidate(
                &subject.id,
                CandidateDecisionRequest {
                    request_id: format!("decision-{}", uuid::Uuid::new_v4()),
                    candidate_id: id.clone(),
                    reason: "Existing confirmed detail remains in its immutable receipt. "
                        .repeat(500),
                    decision: CandidateDecision::Close {
                        action: crate::subjektiv::StagingResolutionAction::Discarded,
                        affected_memory: vec![],
                    },
                },
            )
            .unwrap();
    }
    let generation = store
        .prepare_job_surface_generation(
            &subject.id,
            Some((&job.request.job_id, &attempt.attempt_id)),
        )
        .unwrap();
    let snapshot = store
        .publish_surface_generation(&subject.id, &generation.id, vec![])
        .unwrap();
    let submission = BackendJobResultSubmission {
        job_id: job.request.job_id.clone(),
        attempt_id: attempt.attempt_id.clone(),
        input_revision: job.request.input_revision.clone(),
        result: serde_json::json!({"subject_id":subject.id,"candidate_ids":ids,"surface":{"availability":"ready","generation_id":generation.id,"store_revision":generation.store_revision,"snapshot_id":snapshot.id}}),
    };
    seed_worker_session_for_cleanup(dir.path(), worker);
    let accepted = api.accept_backend_job_result(worker, &submission).unwrap();
    let result = accepted.job.result.as_ref().unwrap();
    assert!(
        serde_json::to_vec(result).unwrap().len() <= job.request.limits.max_result_bytes as usize
    );
    let outcomes = result["candidate_dispositions"].as_array().unwrap();
    assert_eq!(outcomes.len(), ids.len());
    assert!(
        outcomes
            .iter()
            .all(|o| o["action"] == "discarded" && o.get("reason").is_none())
    );
    assert_eq!(
        store.pending_staging_backlog(&subject.id).unwrap().0,
        100 - ids.len()
    );
    assert!(
        api.accept_backend_job_result(worker, &submission)
            .unwrap()
            .replayed
    );
    wait_for_backend_job_worker_cleanup(
        &api,
        &job.request.job_id,
        &attempt.attempt_id,
        BackendJobWorkerCleanupState::Completed,
    )
    .await;
    start_subjektiv_staging_consolidation(
        api.clone(),
        &subject.id,
        MemoryConsolidateStagingOperation { force: true },
    )
    .await
    .unwrap();
    let next = resource(&api, &subject.id);
    assert!(
        next.request
            .grants
            .subjektiv_consolidation
            .unwrap()
            .candidate_ids
            .iter()
            .all(|id| !ids.contains(id))
    );
    assert!(
        store
            .staging_resolution(&subject.id, &ids[0])
            .unwrap()
            .unwrap()
            .reason
            .len()
            > 16 * 1024
    );
}
