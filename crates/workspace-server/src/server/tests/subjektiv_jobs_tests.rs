use super::*;
use crate::subjektiv::{
    CandidateDecision, CandidateDecisionRequest, MemoryChangeTarget, MemoryDraft, SubjectRole,
    SubjectStagingRecord, SurfaceAvailability,
};
use memory::extract::{CandidateKind, ExtractedCandidate, StagingEvidence, StagingRecord};
use memory::schema::SourceRef;
use memory::schema::{EvidenceKind, EvidenceOrigin, EvidenceOriginKind};

const MEMORY_JOB_OPERATIONS: &[&str] = &[
    "memory.consolidation.request",
    "memory.consolidation.supply",
    "memory.staging.list",
    "memory.staging.read",
    "memory.candidate.decide",
    "memory.query",
    "memory.read",
    "memory.changes.list",
    "memory.surface.prepare",
    "memory.surface.supply",
    "memory.surface.publish",
    "memory.surface.fail",
];

pub(super) fn grant_memory_operations(api: &WorkspaceApi, subject: &str, operations: &[&str]) {
    use server_api::authorization::{
        PermissionResource, ScopedPermission, SubjectPermissionsUpdate,
    };
    let store = open_subjektiv_store(api).unwrap();
    let previous = store.subject_permissions(subject).unwrap();
    store
        .set_subject_permissions(
            subject,
            &SubjectPermissionsUpdate {
                operation_id: Uuid::now_v7().to_string(),
                expected_change_id: previous.map(|policy| policy.change_id),
                execution: operations
                    .iter()
                    .map(|operation| ScopedPermission {
                        operation: (*operation).into(),
                        resource: PermissionResource::Subject {
                            workspace_id: TEST_WORKSPACE_ID.into(),
                            subject_id: subject.into(),
                        },
                    })
                    .collect(),
                delegation: vec![],
            },
        )
        .unwrap();
}

async fn memory_job_source(api: &WorkspaceApi, subject: &str) -> RuntimeWorkerRef {
    grant_memory_operations(api, subject, MEMORY_JOB_OPERATIONS);
    attach_memory_job_source(api, subject).await
}

pub(super) async fn attach_memory_job_source(
    api: &WorkspaceApi,
    subject: &str,
) -> RuntimeWorkerRef {
    seed_test_delivery_binding(api);
    let Json(created) = create_workspace_worker(
        State(api.clone()),
        HeaderMap::new(),
        Json(CreateWorkspaceWorkerRequest {
            runtime_id: EMBEDDED_WORKER_RUNTIME_ID.into(),
            display_name: "Memory Job authority source".into(),
            singleton_key: None,
            profile: Some("builtin:companion".into()),
            ticket_assignment: None,
            initial_submit: vec![],
            workdir_attachments: vec![],
            feature_connections: server_api::WorkspaceWorkerFeatureConnectionsRequest {
                subjektiv: Some(server_api::SubjektivWorkerConnectionRequest {
                    subject_id: subject.into(),
                }),
            },
            control_operation_id: None,
            execution_grant: None,
        }),
    )
    .await
    .unwrap();
    RuntimeWorkerRef::new(created.runtime_id, created.worker_id)
}

fn stage(store: &crate::subjektiv::SubjektivStore, subject: &str, id: &str) {
    let origin = EvidenceOrigin {
        kind: EvidenceOriginKind::HumanInput,
        account_id: Some("test-human".into()),
        workspace_id: Some(TEST_WORKSPACE_ID.into()),
        runtime_id: None,
        worker_id: None,
        flow_selector: None,
        flow_definition_id: None,
        flow_definition_fingerprint: None,
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
            authentication_binding_id: "fixture-binding".into(),
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
    let source = memory_job_source(&api, &subject.id).await;
    stage(&store, &subject.id, "candidate-first");
    let started = start_subjektiv_staging_consolidation(
        api.clone(),
        &subject.id,
        Some(source.clone()),
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
        Some(source.clone()),
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
            target: MemoryChangeTarget::Create,
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
    assert_eq!(receipt.memory_fingerprint, replay.memory_fingerprint);
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
        matches!(failed, server_api::SubjektivMemoryBackendResponse::SurfaceFailed(output) if output.status == "failed_confirmed" && output.memory_fingerprint == generation.memory_fingerprint)
    );
    let result = serde_json::json!({"subject_id": subject.id, "candidate_ids": ["candidate-first"], "surface": {"availability":"failed", "generation_id":generation.id, "memory_fingerprint":generation.memory_fingerprint,"reason_code":"editor_failed"}});
    let submission = BackendJobResultSubmission {
        job_id: job.request.job_id.clone(),
        attempt_id: attempt.attempt_id.clone(),
        input_digest: job.request.input_digest().unwrap(),
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
        Some(source.clone()),
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
    let source = memory_job_source(&api, &subject.id).await;
    let old = store.prepare_surface_generation(&subject.id).unwrap();
    assert_eq!(
        start_subjektiv_staging_consolidation(
            api.clone(),
            &subject.id,
            Some(source.clone()),
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
        input_digest: job.request.input_digest().unwrap(),
        result: serde_json::json!({"subject_id":subject.id,"candidate_ids":[],"surface":{"availability":"ready","generation_id":generation.id,"memory_fingerprint":generation.memory_fingerprint,"snapshot_id":snapshot.id}}),
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
            Some(source.clone()),
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
        base_digest: state.snapshot.digest.clone(),
        entrypoints: state.contract.entrypoints.iter().map(|p| p.as_str().into()).collect(),
        changes: vec![server_api::ConfigTreeChange::Update {
            path: entrypoint.as_str().into(), expected_digest: root.content_digest.clone(),
            content: "{ profile = { entries = [{ selector = \"project:job-recipe\"; profile = import \"./profiles/job-recipe.dcdl\"; }]; }; }".into(),
        }, server_api::ConfigTreeChange::Create {
            path: "profiles/job-recipe.dcdl".into(), content_type: server_api::ConfigContentType::Decodal,
            content: "{ slug = \"job-recipe\"; scope = \"workspace_read\"; model = { id = \"fixture-model\"; }; engine = { instruction = \"role.default\"; }; feature = { task = { enabled = true; }; }; }".into(),
        }],
    }).unwrap();
    commit_workspace_config_tree(&api, TEST_WORKSPACE_ID, &config).unwrap();
    let request = BackendJobRequest {
        job_id: "project-recipe-job".into(),
        purpose: "no-profile-switch".into(),
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
    let source = memory_job_source(&api, &subject.id).await;
    let subject_body = source.clone();
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
                resolved_execution_grant: None,
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
            Some(source.clone()),
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
            Some(source.clone()),
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
    let source = memory_job_source(&api, &subject.id).await;
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
        Some(source.clone()),
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
        input_digest: job.request.input_digest().unwrap(),
        result: serde_json::json!({"subject_id":subject.id,"candidate_ids":ids,"surface":{"availability":"ready","generation_id":generation.id,"memory_fingerprint":generation.memory_fingerprint,"snapshot_id":snapshot.id}}),
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
        Some(source.clone()),
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

#[tokio::test]
async fn subjektiv_job_rechecks_current_policy_and_preserves_provider_batch_ceiling() {
    let dir = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(dir.path()).await;
    let store = open_subjektiv_store(&api).unwrap();
    let subject = store
        .create_subject(SubjectRole::new("live-job-policy").unwrap())
        .unwrap();
    let source = memory_job_source(&api, &subject.id).await;
    let mut operations = MEMORY_JOB_OPERATIONS.to_vec();
    operations.extend(["journal.state.read", "session.record"]);
    grant_memory_operations(&api, &subject.id, &operations);
    stage(&store, &subject.id, "frozen-candidate");
    start_subjektiv_staging_consolidation(
        api.clone(),
        &subject.id,
        Some(source.clone()),
        MemoryConsolidateStagingOperation { force: true },
    )
    .await
    .unwrap();
    let job = resource(&api, &subject.id);
    assert_eq!(job.request.source_worker.as_ref(), Some(&source));
    let attempt = current_attempt(&api, &job);
    let worker = attempt.worker.as_ref().unwrap();
    assert_eq!(execution.take_inputs().len(), 1);
    for operation in ["journal.state.read", "session.record"] {
        let request = server_api::authorization::OperationAuthorizationRequest {
            operation: operation.into(),
            resource: server_api::authorization::PermissionResource::Subject {
                workspace_id: TEST_WORKSPACE_ID.into(),
                subject_id: subject.id.clone(),
            },
        };
        assert!(subject_authorization::authorize_scope(&api, &source, &request).is_ok());
        assert!(
            subject_authorization::authorize_scope(&api, worker, &request).is_err(),
            "current body policy cannot widen a consolidation Job's domain: {operation}"
        );
    }
    let prepare = || {
        server_api::SubjektivMemoryBackendOperation::PrepareSurface(
            server_api::SubjektivSurfacePrepareRequest {},
        )
    };
    // Late arrivals never enter this attempt's frozen batch, and the pending
    // granted candidate still prevents premature surface preparation.
    stage(&store, &subject.id, "late-candidate");
    assert!(
        subjektiv_job::require_candidate_grant(&api, TEST_WORKSPACE_ID, worker, "frozen-candidate")
            .is_ok()
    );
    assert!(
        subjektiv_job::require_candidate_grant(&api, TEST_WORKSPACE_ID, worker, "late-candidate")
            .is_err()
    );
    let read_candidate = |id: &str| {
        server_api::SubjektivMemoryBackendOperation::ReadCandidate(
            server_api::SubjektivMemoryCandidateReadRequest {
                candidate_id: id.into(),
            },
        )
    };
    assert!(
        op(&api, worker, read_candidate("frozen-candidate"))
            .await
            .is_ok()
    );
    assert!(
        op(&api, worker, read_candidate("late-candidate"))
            .await
            .is_err()
    );
    grant_memory_operations(&api, &subject.id, &["memory.surface.prepare"]);
    let error = op(&api, worker, read_candidate("frozen-candidate"))
        .await
        .unwrap_err();
    assert!(
        error.error.to_string().contains("memory.staging.read"),
        "{error:?}"
    );
    let permissions = subject_authorization::worker_permissions(&api, worker)
        .unwrap()
        .unwrap();
    assert_eq!(permissions.len(), 1);
    assert_eq!(permissions[0].operation, "memory.surface.prepare");
    // Provider batch checks remain active even when operation policy allows it.
    assert!(op(&api, worker, prepare()).await.is_err());
    grant_memory_operations(&api, &subject.id, &[]);
    let error = op(&api, worker, prepare()).await.unwrap_err();
    assert!(
        error.error.to_string().contains("memory.surface.prepare"),
        "{error:?}"
    );
    assert!(
        subject_authorization::worker_permissions(&api, worker)
            .unwrap()
            .unwrap()
            .is_empty()
    );
    assert_eq!(resource(&api, &subject.id).request, job.request);
    assert!(execution.take_inputs().is_empty());
}

#[tokio::test]
async fn saved_job_without_frozen_source_cannot_use_non_subject_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let (api, _) = test_api_with_recording_backend(dir.path()).await;
    let store = open_subjektiv_store(&api).unwrap();
    let subject = store
        .create_subject(SubjectRole::new("legacy-job-origin").unwrap())
        .unwrap();
    let _source = memory_job_source(&api, &subject.id).await;
    // Reconstruct a retained pre-migration dispatched attempt through the ledger,
    // not today's dispatcher: new dispatch itself must reject a missing origin.
    let request = BackendJobRequest {
        job_id: "legacy-no-operation-origin".into(),
        purpose: "subjektiv_consolidation".into(),
        input_ref: format!("subjektiv://{}/consolidation", subject.id),
        input: serde_json::json!({"subject_id":subject.id,"candidate_ids":[]}),
        instruction: "retained legacy input".into(),
        profile: "builtin:subjektiv-memory-consolidation".into(),
        grants: crate::backend_job::BackendJobGrants {
            subjektiv_consolidation: Some(crate::backend_job::SubjektivConsolidationGrant {
                subject_id: subject.id.clone(),
                candidate_ids: vec![],
            }),
            ..Default::default()
        },
        serialization_key: None,
        source_worker: None,
        notification_target: None,
        limits: Default::default(),
    };
    let error = subject_authorization::authorize_job_input(&api, &request).unwrap_err();
    assert!(error.error.to_string().contains("frozen"), "{error:?}");
    let now = Utc::now().to_rfc3339();
    let reserved = api
        .store
        .reserve_backend_job(TEST_WORKSPACE_ID, &request, &now)
        .unwrap();
    let (_, claimed) = api
        .store
        .claim_backend_job_attempt_dispatch(
            TEST_WORKSPACE_ID,
            &request.job_id,
            &reserved.attempt.attempt_id,
            &now,
        )
        .unwrap();
    assert!(claimed);
    let retained_worker = spawn_ticket_check_source(&api, "retained-legacy-job");
    api.store
        .bind_backend_job_attempt_worker(
            TEST_WORKSPACE_ID,
            &request.job_id,
            &reserved.attempt.attempt_id,
            &retained_worker,
            None,
            &now,
        )
        .unwrap();
    let job = resource(&api, &subject.id);
    let attempt = current_attempt(&api, &job);
    let worker = attempt.worker.as_ref().unwrap();
    assert!(subjektiv_job::active_grant(&api, TEST_WORKSPACE_ID, worker).is_ok());
    let error = op(
        &api,
        worker,
        server_api::SubjektivMemoryBackendOperation::PrepareSurface(
            server_api::SubjektivSurfacePrepareRequest {},
        ),
    )
    .await
    .unwrap_err();
    assert!(error.error.to_string().contains("frozen"), "{error:?}");
    assert!(subject_authorization::worker_permissions(&api, worker).is_err());
    assert_eq!(resource(&api, &subject.id).request, job.request);
}

#[tokio::test]
async fn unset_subject_consolidation_denies_before_job_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(dir.path()).await;
    let store = open_subjektiv_store(&api).unwrap();
    let subject = store
        .create_subject(SubjectRole::new("unset-job-policy").unwrap())
        .unwrap();
    let source = attach_memory_job_source(&api, &subject.id).await;
    assert!(store.subject_permissions(&subject.id).unwrap().is_none());
    stage(&store, &subject.id, "untouched");
    let error = start_subjektiv_staging_consolidation(
        api.clone(),
        &subject.id,
        Some(source),
        MemoryConsolidateStagingOperation { force: true },
    )
    .await
    .unwrap_err();
    assert!(
        error
            .error
            .to_string()
            .contains("memory.consolidation.request"),
        "{error:?}"
    );
    assert!(execution.take_inputs().is_empty());
    assert!(
        api.store
            .find_active_backend_job_for_resource(
                TEST_WORKSPACE_ID,
                &crate::backend_job::subjektiv_serialization_key(&subject.id)
            )
            .unwrap()
            .is_none()
    );
    assert_eq!(store.pending_staging_backlog(&subject.id).unwrap().0, 1);
}

#[tokio::test]
async fn subject_consolidation_request_without_supply_denies_before_model_input() {
    let dir = tempfile::tempdir().unwrap();
    let (api, execution) = test_api_with_recording_backend(dir.path()).await;
    let store = open_subjektiv_store(&api).unwrap();
    let subject = store
        .create_subject(SubjectRole::new("job-supply-denied").unwrap())
        .unwrap();
    let source = memory_job_source(&api, &subject.id).await;
    stage(&store, &subject.id, "not-supplied");
    grant_memory_operations(&api, &subject.id, &["memory.consolidation.request"]);
    let error = start_subjektiv_staging_consolidation(
        api.clone(),
        &subject.id,
        Some(source),
        MemoryConsolidateStagingOperation { force: true },
    )
    .await
    .unwrap_err();
    assert!(
        error
            .error
            .to_string()
            .contains("memory.consolidation.supply"),
        "{error:?}"
    );
    assert!(
        execution.take_inputs().is_empty(),
        "denied frozen input must not reach the model/runtime"
    );
    let saved = resource(&api, &subject.id);
    let attempt = current_attempt(&api, &saved);
    assert_eq!(attempt.state, BackendJobAttemptState::Reserved);
    assert!(
        attempt.worker.is_none(),
        "supply refusal must precede claim/allocation"
    );
    assert_eq!(
        saved.request.input["candidate_ids"],
        serde_json::json!(["not-supplied"])
    );
    assert_eq!(store.pending_staging_backlog(&subject.id).unwrap().0, 1);
}
