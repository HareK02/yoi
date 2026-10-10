use super::*;
use crate::{MemoryDraft, SubjectRole, SubjectSessionAttribution, SubjektivStore};
use memory::extract::CandidateKind;
use server_api::{
    SubjektivMemoryBackendOperation as Op, SubjektivMemoryBackendResponse as Response,
};

fn open(root: &std::path::Path, scope: &str) -> (feature_storage::FeatureStorage, SubjektivStore) {
    let manager = feature_storage::FeatureStorage::new(root);
    let scope = manager.scope(scope).unwrap();
    let registration = SubjektivStore::register(&scope).unwrap();
    let store = SubjektivStore::open(&scope, &registration).unwrap();
    (manager, store)
}

fn query(cursor: Option<String>) -> Op {
    Op::Query(server_api::SubjektivMemoryQueryRequest {
        query: None,
        kinds: None,
        states: None,
        limit: Some(1),
        cursor,
    })
}

fn read(
    id: &str,
    change_id: Option<String>,
    offset: Option<usize>,
    byte_offset: Option<usize>,
) -> Op {
    Op::Read(server_api::SubjektivMemoryReadRequest {
        memory_id: id.into(),
        change_id,
        offset,
        byte_offset,
        limit: Some(1),
        evidence_cursor: None,
    })
}

#[test]
fn neutral_scope_reopen_and_context_attenuation() {
    let temp = tempfile::tempdir().unwrap();
    let random_scope = uuid::Uuid::now_v7().to_string();
    let (manager, store) = open(temp.path(), &random_scope);
    assert_eq!(store.scope_id(), random_scope);
    let subject = store
        .create_subject(SubjectRole::new("companion").unwrap())
        .unwrap();
    let body = HostOperationContext::validated_body(&store, &subject.id, None).unwrap();
    let consolidation =
        HostOperationContext::validated_consolidation(&store, &subject.id, Some(vec![]), None)
            .unwrap();
    assert!(
        execute(
            &store,
            &body,
            Op::ListCandidates(server_api::SubjektivMemoryCandidateListRequest { limit: None })
        )
        .is_err()
    );
    assert!(
        execute(
            &store,
            &consolidation,
            Op::ResidentSummary(memory::backend::MemoryResidentSummaryOperation {})
        )
        .is_err()
    );
    let attribution =
        SubjectSessionAttribution::new(&subject.id, "local-host", "worker", "session").unwrap();
    assert!(
        HostOperationContext::validated_body(&store, &subject.id, Some(attribution.clone()))
            .is_err()
    );
    store
        .record_session_attribution(attribution.clone())
        .unwrap();
    HostOperationContext::validated_body(&store, &subject.id, Some(attribution)).unwrap();
    let (_, foreign) = open(&temp.path().join("foreign"), "foreign-scope");
    assert!(matches!(
        execute(&foreign, &body, query(None)),
        Err(OperationError::PermissionDenied(_))
    ));
    manager.shutdown().unwrap();
    drop(store);
    let (_manager, reopened) = open(temp.path(), &random_scope);
    assert_eq!(reopened.subject(&subject.id).unwrap().unwrap(), subject);
    assert!(reopened.session_attribution("session").unwrap().is_some());
}

#[test]
fn shared_query_cursor_and_utf8_fixed_change_projection() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, store) = open(temp.path(), "local-random-scope");
    let subject = store
        .create_subject(SubjectRole::new("companion").unwrap())
        .unwrap();
    let context = HostOperationContext::validated_body(&store, &subject.id, None).unwrap();
    let original = "日本語\\\"".repeat(10_000);
    let first = store
        .create_memory(
            &subject.id,
            MemoryDraft::active(
                CandidateKind::Lesson,
                "first",
                &original,
                "reason",
                "initial",
            ),
        )
        .unwrap();
    store
        .create_memory(
            &subject.id,
            MemoryDraft::active(
                CandidateKind::Decision,
                "second",
                "second body",
                "reason",
                "initial",
            ),
        )
        .unwrap();
    let Response::Query(page) = execute(&store, &context, query(None)).unwrap() else {
        panic!()
    };
    assert!(page.has_more);
    assert_eq!(page.items.len(), 1);
    let cursor = page.next_cursor.unwrap();
    store
        .create_memory(
            &subject.id,
            MemoryDraft::active(
                CandidateKind::Constraint,
                "third",
                "third body",
                "reason",
                "initial",
            ),
        )
        .unwrap();
    assert!(matches!(
        execute(&store, &context, query(Some(cursor))),
        Err(OperationError::Conflict(_))
    ));
    assert!(execute(&store, &context, read(&first.id, None, Some(1), None)).is_err());
    let mut offset = None;
    let mut byte_offset = None;
    let mut reconstructed = String::new();
    loop {
        let Response::Read(page) = execute(
            &store,
            &context,
            read(
                &first.id,
                Some(first.change_id.clone()),
                offset,
                byte_offset,
            ),
        )
        .unwrap() else {
            panic!()
        };
        assert!(
            serde_json::to_vec_pretty(&page).unwrap().len()
                <= server_api::SUBJEKTIV_MEMORY_READ_MAX_TOOL_CONTENT_BYTES
        );
        reconstructed.push_str(&page.body_md);
        if !page.body_truncated {
            break;
        }
        assert!(!page.body_md.is_empty());
        offset = page.body_next_offset;
        byte_offset = page.body_next_byte_offset;
    }
    assert_eq!(reconstructed, original);
}

#[test]
fn shared_surface_empty_snapshot_is_ready_not_missing() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, store) = open(temp.path(), "local-random-scope");
    let subject = store
        .create_subject(SubjectRole::new("companion").unwrap())
        .unwrap();
    let consolidation = HostOperationContext::validated_consolidation(
        &store,
        &subject.id,
        Some(vec![]),
        Some(JobAttemptBinding {
            job_id: "job".into(),
            attempt_id: "attempt".into(),
        }),
    )
    .unwrap();
    let prepared: server_api::SubjektivSurfacePrepareRequest = serde_json::from_str("{}").unwrap();
    let Response::SurfacePrepared(prepared) =
        execute(&store, &consolidation, Op::PrepareSurface(prepared)).unwrap()
    else {
        panic!()
    };
    let Response::SurfacePublished(published) = execute(
        &store,
        &consolidation,
        Op::PublishSurface(server_api::SubjektivSurfacePublishRequest {
            generation_id: prepared.generation_id,
            points: vec![],
        }),
    )
    .unwrap() else {
        panic!()
    };
    assert!(published.empty);
    let body = HostOperationContext::validated_body(&store, &subject.id, None).unwrap();
    let Response::ResidentSummary(resident) = execute(
        &store,
        &body,
        Op::ResidentSummary(memory::backend::MemoryResidentSummaryOperation {}),
    )
    .unwrap() else {
        panic!()
    };
    assert_eq!(
        resident.availability,
        memory::backend::MemoryResidentSummaryAvailability::Ready
    );
    assert_eq!(resident.content, None);
}

#[test]
fn explicit_receipts_candidate_batch_and_decision_retries_share_logic() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, store) = open(temp.path(), "random-local-scope");
    let subject = store
        .create_subject(SubjectRole::new("companion").unwrap())
        .unwrap();
    let attribution =
        SubjectSessionAttribution::new(&subject.id, "host", "worker", "session").unwrap();
    store
        .record_session_attribution(attribution.clone())
        .unwrap();
    let context =
        HostOperationContext::validated_body(&store, &subject.id, Some(attribution)).unwrap();
    // Host-resolved public Session evidence with no invented Workspace identity.
    let input = server_api::SubjektivMemoryStageExplicitRequest {
        receipt_id: "receipt-1".into(),
        session_id: "session".into(),
        kind: CandidateKind::Lesson,
        claim: "Preserve correction history".into(),
        why_useful: "Allows auditing".into(),
        staleness: None,
        proposal: None,
        evidence: vec![memory::extract::StagingEvidence {
            id: "e1".into(),
            entry_range: Some([1, 1]),
            kind: memory::schema::EvidenceKind::new("model_output"),
            origin: None,
            excerpt: Some("Use immutable changes".into()),
            summary: None,
        }],
        source_refs: vec![memory::schema::SourceEvidenceRef {
            session_id: Some("session".into()),
            segment_id: Some("segment".into()),
            entry_range: Some([1, 1]),
            evidence_id: Some("e1".into()),
            ..Default::default()
        }],
    };
    let mut foreign_session = input.clone();
    foreign_session.session_id = "foreign".into();
    assert!(matches!(
        execute(&store, &context, Op::StageExplicit(foreign_session)),
        Err(OperationError::PermissionDenied(_))
    ));
    execute(&store, &context, Op::StageExplicit(input.clone())).unwrap();
    execute(&store, &context, Op::StageExplicit(input)).unwrap();
    let Response::ReceiptStatus(receipt) = execute(
        &store,
        &context,
        Op::ReceiptStatus(server_api::SubjektivMemoryReceiptStatusRequest {
            receipt_id: "receipt-1".into(),
        }),
    )
    .unwrap() else {
        panic!()
    };
    assert_eq!(
        receipt.status,
        server_api::SubjektivMemoryReceiptStatus::Staged
    );
    let candidate_id = receipt.candidate_id.unwrap();
    let no_batch =
        HostOperationContext::validated_consolidation(&store, &subject.id, Some(vec![]), None)
            .unwrap();
    assert!(matches!(
        execute(
            &store,
            &no_batch,
            Op::ReadCandidate(server_api::SubjektivMemoryCandidateReadRequest {
                candidate_id: candidate_id.clone()
            })
        ),
        Err(OperationError::PermissionDenied(_))
    ));
    let batch = HostOperationContext::validated_consolidation(
        &store,
        &subject.id,
        Some(vec![candidate_id.clone()]),
        None,
    )
    .unwrap();
    assert!(
        execute(
            &store,
            &batch,
            Op::PrepareSurface(serde_json::from_str("{}").unwrap())
        )
        .is_err()
    );
    let decision = server_api::SubjektivMemoryCandidateDecisionRequest {
        request_id: "decision-1".into(),
        candidate_id,
        reason: "adopt lesson".into(),
        decision: server_api::SubjektivMemoryCandidateDecision::Apply {
            target: server_api::SubjektivMemoryApplyTarget::Create,
            memory: server_api::SubjektivMemoryDraft {
                kind: CandidateKind::Lesson,
                state: server_api::SubjektivMemoryState::Active,
                claim: "Preserve correction history".into(),
                body_md: "Use immutable changes".into(),
                why_useful: "Allows auditing".into(),
                staleness: None,
                derived_from: vec![],
                change_reason: "adopt".into(),
            },
        },
    };
    let first = execute(&store, &batch, Op::DecideCandidate(decision.clone())).unwrap();
    let retry = execute(&store, &batch, Op::DecideCandidate(decision)).unwrap();
    assert_eq!(
        serde_json::to_value(first).unwrap(),
        serde_json::to_value(retry).unwrap()
    );
    assert_eq!(store.list_memories(&subject.id).unwrap().len(), 1);
    execute(
        &store,
        &batch,
        Op::PrepareSurface(serde_json::from_str("{}").unwrap()),
    )
    .unwrap();
}

#[test]
fn history_pages_pin_ancestry_and_reject_unknown_heads() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, store) = open(temp.path(), "scope");
    let subject = store
        .create_subject(SubjectRole::new("companion").unwrap())
        .unwrap();
    let draft = |body: &str| {
        MemoryDraft::active(CandidateKind::Lesson, "claim", body, "useful", "evidence")
    };
    let first = store.create_memory(&subject.id, draft("first")).unwrap();
    let second = store
        .revise_memory(
            &subject.id,
            &first.id,
            first.change_id.clone(),
            draft("second"),
        )
        .unwrap();
    let input = |cursor| server_api::SubjektivMemoryListChangesRequest {
        memory_id: first.id.clone(),
        limit: Some(1),
        cursor,
    };
    let page = subjektiv_memory_list_changes(&store, &subject.id, input(None)).unwrap();
    assert_eq!(page.items[0].change_id, second.change_id);
    assert!(page.has_more);
    let third = store
        .revise_memory(
            &subject.id,
            &first.id,
            second.change_id.clone(),
            draft("third"),
        )
        .unwrap();
    let next = subjektiv_memory_list_changes(&store, &subject.id, input(page.next_cursor)).unwrap();
    assert_eq!(next.items[0].change_id, first.change_id);
    assert_eq!(next.current_change_id, third.change_id);
    assert!(!next.has_more);
    let invalid = encode_subjektiv_cursor(
        "changes",
        &SubjektivChangeCursor {
            subject_id: subject.id.clone(),
            memory_id: first.id.clone(),
            head_change_id: "not-a-history-entry".into(),
            offset: 0,
        },
    )
    .unwrap();
    assert!(subjektiv_memory_list_changes(&store, &subject.id, input(Some(invalid))).is_err());
    let historic = subjektiv_memory_read(
        &store,
        &subject.id,
        server_api::SubjektivMemoryReadRequest {
            memory_id: first.id,
            change_id: Some(second.change_id.clone()),
            offset: None,
            byte_offset: None,
            limit: None,
            evidence_cursor: None,
        },
    )
    .unwrap();
    assert_eq!(historic.body_md, "second");
    assert_eq!(historic.change_id, second.change_id);
    assert_eq!(historic.current_change_id, third.change_id);
}

#[test]
fn query_content_condition_ignores_behavior_and_detects_aba_memory_writes() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, store) = open(temp.path(), "scope");
    let subject = store
        .create_subject(SubjectRole::new("companion").unwrap())
        .unwrap();
    let draft = |body: &str| {
        MemoryDraft::active(CandidateKind::Lesson, "claim", body, "useful", "evidence")
    };
    let first = store.create_memory(&subject.id, draft("A")).unwrap();
    store.create_memory(&subject.id, draft("other")).unwrap();
    let context = HostOperationContext::validated_body(&store, &subject.id, None).unwrap();
    let Response::Query(page) = execute(&store, &context, query(None)).unwrap() else {
        panic!()
    };
    let cursor = page.next_cursor.unwrap();
    store
        .update_subject_behavior(&subject.id, "", "New behavior".into())
        .unwrap();
    execute(&store, &context, query(Some(cursor.clone()))).unwrap();
    let second = store
        .revise_memory(&subject.id, &first.id, first.change_id.clone(), draft("B"))
        .unwrap();
    store
        .revise_memory(&subject.id, &first.id, second.change_id, draft("A"))
        .unwrap();
    assert!(matches!(
        execute(&store, &context, query(Some(cursor))),
        Err(OperationError::Conflict(_))
    ));
}
