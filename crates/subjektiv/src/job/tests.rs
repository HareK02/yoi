use super::*;
use crate::{
    CandidateDecision, CandidateDecisionRequest, StagingResolutionAction, SubjectRole,
    SubjectStagingRecord,
};
use memory::extract::{CandidateKind, ExtractedCandidate, StagingRecord};
use memory::schema::SourceRef;

#[test]
fn abandoned_attempt_inspection_preserves_original_preparation_and_rejects_stale_input() {
    let temp = tempfile::tempdir().unwrap();
    let manager = feature_storage::FeatureStorage::new(temp.path());
    let scope = manager.scope("abandoned-consolidation").unwrap();
    let registration = SubjektivStore::register(&scope).unwrap();
    let store = SubjektivStore::open(&scope, &registration).unwrap();
    let subject = store
        .create_subject(SubjectRole::new("companion").unwrap())
        .unwrap();
    assert_eq!(
        inspect_abandoned_attempt(&store, &subject.id, &[], "job", "attempt").unwrap(),
        AbandonedAttempt {
            disposition_ids: vec![],
            unresolved_candidate_ids: vec![],
            prepared_generation_id: None,
        }
    );
    let generation = store
        .prepare_job_surface_generation(&subject.id, Some(("job", "attempt")))
        .unwrap();
    let surface = store.resident_surface(&subject.id).unwrap();
    for _ in 0..2 {
        let abandoned =
            inspect_abandoned_attempt(&store, &subject.id, &[], "job", "attempt").unwrap();
        assert_eq!(
            abandoned.prepared_generation_id.as_deref(),
            Some(generation.id.as_str())
        );
        // Inspection never fabricates a terminal surface failure or finalizes it.
        assert!(
            lookup_result(&store, &subject.id, &[], "job", "attempt")
                .unwrap()
                .is_none()
        );
        assert_eq!(store.resident_surface(&subject.id).unwrap(), surface);
    }
    let count: i64 = store
        .database
        .try_with_connection(|connection| {
            Ok::<_, crate::SubjektivError>(connection.query_row(
                "SELECT COUNT(*) FROM surface_generation_runs",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(count, 1, "no new generation on abandoned reconciliation");
    let ready = store
        .publish_surface_generation(&subject.id, &generation.id, vec![])
        .unwrap();
    inspect_abandoned_attempt(&store, &subject.id, &[], "job", "attempt").unwrap();
    assert_eq!(
        store.resident_surface(&subject.id).unwrap().snapshot,
        Some(ready)
    );
    assert!(
        inspect_abandoned_attempt(&store, &subject.id, &["missing".into()], "job", "attempt")
            .is_err()
    );
    store
        .create_memory(
            &subject.id,
            crate::MemoryDraft::active(CandidateKind::Lesson, "new", "body", "reason", "change"),
        )
        .unwrap();
    assert!(inspect_abandoned_attempt(&store, &subject.id, &[], "job", "attempt").is_err());
    store
        .begin_subject_deletion(&subject.id, "delete-abandoned-subject")
        .unwrap();
    assert!(
        inspect_abandoned_attempt(&store, &subject.id, &[], "fresh-job", "fresh-attempt").is_err()
    );
}

#[test]
fn prepared_dirty_surface_waits_for_terminal_receipt_and_rejects_stale_input() {
    let temp = tempfile::tempdir().unwrap();
    let manager = feature_storage::FeatureStorage::new(temp.path());
    let scope = manager.scope("consolidation-recovery").unwrap();
    let registration = SubjektivStore::register(&scope).unwrap();
    let store = SubjektivStore::open(&scope, &registration).unwrap();
    let subject = store
        .create_subject(SubjectRole::new("companion").unwrap())
        .unwrap();
    store
        .prepare_job_surface_generation(&subject.id, Some(("old-job", "old-attempt")))
        .unwrap();
    store
        .create_memory(
            &subject.id,
            crate::MemoryDraft::active(CandidateKind::Lesson, "lesson", "body", "reason", "change"),
        )
        .unwrap();
    let prepared = store
        .prepare_job_surface_generation(&subject.id, Some(("job", "attempt")))
        .unwrap();
    // A committed preparation is not a terminal receipt, even with no candidates.
    assert!(
        lookup_result(&store, &subject.id, &[], "job", "attempt")
            .unwrap()
            .is_none()
    );
    assert!(lookup_result(&store, &subject.id, &[], "old-job", "old-attempt").is_err());
    store
        .fail_surface_generation(&subject.id, &prepared.id, "model_failure")
        .unwrap();
    let completed = lookup_result(&store, &subject.id, &[], "job", "attempt")
        .unwrap()
        .unwrap();
    assert_eq!(completed["surface"]["generation_id"], prepared.id);
    assert_eq!(completed["surface"]["availability"], "failed");
    assert_eq!(completed["surface"]["reason_code"], "model_failure");
}

#[test]
fn escaped_ids_use_exact_t704_complete_result_budget() {
    const MAX_RESULT_BYTES: u32 = 16 * 1024;
    let ids: Vec<_> = (0..100)
        .map(|i| format!("{i:03}{}", "\\\"".repeat(126)))
        .collect();
    let batch = bounded_candidate_batch(ids.clone(), MAX_RESULT_BYTES).unwrap();
    assert!(!batch.is_empty());
    assert!(batch.len() < ids.len());
    assert_eq!(batch, ids[..batch.len()]);
    let dispositions: Vec<_> = batch
        .iter()
        .map(|id| serde_json::json!({"candidate_id": id, "action": "already_covered"}))
        .collect();
    let result = serde_json::json!({
        "subject_id": "\\\"".repeat(128), "candidate_ids": batch, "candidate_dispositions": dispositions,
        "surface": { "availability": "failed", "generation_id": "g".repeat(100), "memory_fingerprint": "f".repeat(64), "reason_code": "\\\"".repeat(128) }
    });
    assert!(serde_json::to_vec(&result).unwrap().len() <= MAX_RESULT_BYTES as usize);
    assert!(
        bounded_candidate_batch(["one".into()], 2048)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        bounded_candidate_batch((0..200).map(|i| i.to_string()), u32::MAX)
            .unwrap()
            .len(),
        CANDIDATE_BATCH_LIMIT
    );
}

#[test]
fn trigger_policy_preserves_empty_ready_and_surface_retry_rules() {
    for availability in [
        SurfaceAvailability::Ungenerated,
        SurfaceAvailability::Stale,
        SurfaceAvailability::Failed,
    ] {
        assert!(consolidation_required(0, 0, false, availability));
        assert!(consolidation_required(1, 1, false, availability));
    }
    assert!(!consolidation_required(
        0,
        0,
        true,
        SurfaceAvailability::Ready
    ));
    assert!(!consolidation_required(
        4,
        49_999,
        false,
        SurfaceAvailability::Ready
    ));
    assert!(consolidation_required(
        5,
        0,
        false,
        SurfaceAvailability::Ready
    ));
    assert!(consolidation_required(
        1,
        50_000,
        false,
        SurfaceAvailability::Ready
    ));
    assert!(consolidation_required(
        1,
        1,
        true,
        SurfaceAvailability::Ready
    ));
}

#[test]
fn result_checks_durable_dispositions_and_exact_job_surface_outcome() {
    let temp = tempfile::tempdir().unwrap();
    let manager = feature_storage::FeatureStorage::new(temp.path());
    let scope = manager.scope("local-random-scope").unwrap();
    let registration = SubjektivStore::register(&scope).unwrap();
    let store = SubjektivStore::open(&scope, &registration).unwrap();
    let subject = store
        .create_subject(SubjectRole::new("companion").unwrap())
        .unwrap();
    let candidate = SubjectStagingRecord::attach(
        &subject.id,
        StagingRecord::from_candidate(
            "candidate",
            "dedup",
            SourceRef {
                segment_id: "segment".into(),
                range: [1, 1],
            },
            ExtractedCandidate {
                kind: CandidateKind::Lesson,
                claim: "claim".into(),
                why_useful: "reason".into(),
                staleness: None,
                evidence_ids: vec![],
            },
            vec![],
            vec![],
        ),
    );
    store.stage_candidate(candidate).unwrap();
    let batch = vec!["candidate".to_string()];
    let generation = store
        .prepare_job_surface_generation(&subject.id, Some(("job", "attempt")))
        .unwrap();
    let snapshot = store
        .publish_surface_generation(&subject.id, &generation.id, vec![])
        .unwrap();
    let ready = serde_json::json!({"subject_id": subject.id, "candidate_ids": batch,
        "surface": {"availability": "ready", "generation_id": generation.id, "memory_fingerprint": snapshot.built_from_memory_fingerprint, "snapshot_id": snapshot.id}});
    assert!(
        validate_result(&store, &subject.id, &batch, "job", "attempt", &ready)
            .unwrap_err()
            .to_string()
            .contains("unresolved")
    );
    assert!(with_dispositions(&store, &subject.id, &batch, &ready).is_err());
    assert!(
        lookup_result(&store, &subject.id, &batch, "job", "attempt")
            .unwrap()
            .is_none()
    );
    store
        .decide_candidate(
            &subject.id,
            CandidateDecisionRequest {
                request_id: "decision".into(),
                candidate_id: "candidate".into(),
                reason: "invalid".into(),
                decision: CandidateDecision::Close {
                    action: StagingResolutionAction::Invalid,
                    affected_memory: vec![],
                },
            },
        )
        .unwrap();
    validate_result(&store, &subject.id, &batch, "job", "attempt", &ready).unwrap();
    let enriched = with_dispositions(&store, &subject.id, &batch, &ready).unwrap();
    assert_eq!(
        lookup_result(&store, &subject.id, &batch, "job", "attempt").unwrap(),
        Some(enriched.clone())
    );
    assert_eq!(
        enriched["candidate_dispositions"],
        serde_json::json!([{"candidate_id": "candidate", "action": "invalid"}])
    );
    assert_eq!(
        enriched,
        with_dispositions(&store, &subject.id, &batch, &ready).unwrap()
    );
    assert!(validate_result(&store, &subject.id, &batch, "job", "other-attempt", &ready).is_err());
    assert!(validate_result(&store, &subject.id, &[], "job", "attempt", &ready).is_err());
    // A current ready snapshot survives a redundant editor failure. Advance
    // Memory first so this attempt genuinely needs a new surface and records failure.
    store
        .create_memory(
            &subject.id,
            crate::MemoryDraft::active(
                CandidateKind::Lesson,
                "new lesson",
                "new body",
                "reason",
                "change",
            ),
        )
        .unwrap();
    let failure = store
        .prepare_job_surface_generation(&subject.id, Some(("job", "attempt")))
        .unwrap();
    let memory_fingerprint = store
        .fail_surface_generation(&subject.id, &failure.id, "model_failure")
        .unwrap();
    let failed = serde_json::json!({"subject_id": subject.id, "candidate_ids": batch,
        "surface": {"availability": "failed", "generation_id": failure.id, "memory_fingerprint": memory_fingerprint, "reason_code": "model_failure"}});
    validate_result(&store, &subject.id, &batch, "job", "attempt", &failed).unwrap();
    // The same attempt can prepare again after committing Memory. Recovery must
    // select its compatible stored outcome, not its first/last-arriving generation.
    assert_eq!(
        lookup_result(&store, &subject.id, &batch, "job", "attempt").unwrap(),
        Some(with_dispositions(&store, &subject.id, &batch, &failed).unwrap())
    );
    assert!(validate_result(&store, &subject.id, &batch, "job", "attempt", &ready).is_err());
    let mut false_reason = failed;
    false_reason["surface"]["reason_code"] = serde_json::json!("invented");
    assert!(validate_result(&store, &subject.id, &batch, "job", "attempt", &false_reason).is_err());
}
