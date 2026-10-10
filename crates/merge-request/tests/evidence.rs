use chrono::{TimeZone, Utc};
use merge_request::*;

fn actor() -> WorkerIdentity {
    WorkerIdentity {
        runtime_id: "runtime".into(),
        worker_id: "reviewer".into(),
    }
}

fn subjects() -> Vec<MergeRequestReviewSubject> {
    vec![
        MergeRequestReviewSubject {
            merge_request_id: "MR-1".into(),
            subject_ref: "source-one".into(),
        },
        MergeRequestReviewSubject {
            merge_request_id: "MR-2".into(),
            subject_ref: "source-two".into(),
        },
    ]
}

fn add_review(
    request: &mut MergeRequest,
    event_id: &str,
    digest: &str,
    snapshot: &[MergeRequestReviewSubject],
    decision: ReviewDecision,
) {
    let subject = snapshot
        .iter()
        .find(|subject| subject.merge_request_id == request.merge_request_id)
        .unwrap();
    let sequence = request.thread.len() as u64 + 1;
    let requested = ReviewRequestedEvent {
        event_id: format!("request-{event_id}"),
        sequence,
        subject_ref: subject.subject_ref.clone(),
        ticket_content_digest: digest.into(),
        ticket_merge_request_subjects: snapshot.to_vec(),
        requested_by: actor(),
        reviewer: actor(),
        created_at: request.created_at,
    };
    let review = ReviewEvent {
        event_id: event_id.into(),
        sequence: sequence + 1,
        request_event_id: requested.event_id.clone(),
        subject_ref: requested.subject_ref.clone(),
        ticket_content_digest: digest.into(),
        ticket_merge_request_subjects: snapshot.to_vec(),
        decision,
        body: String::new(),
        findings: vec![],
        reviewer: actor(),
        created_at: request.created_at,
    };
    request
        .thread
        .push(MergeRequestThreadEvent::ReviewRequested(requested));
    request.thread.push(MergeRequestThreadEvent::Review(review));
}

fn merged_requests() -> Vec<MergeRequest> {
    let now = Utc.with_ymd_and_hms(2026, 10, 8, 12, 0, 0).unwrap();
    let snapshot = subjects();
    snapshot
        .iter()
        .map(|subject| {
            let mut request = MergeRequest {
                workspace_id: "W".into(),
                merge_request_id: subject.merge_request_id.clone(),
                repository_id: subject.merge_request_id.clone(),
                state: MergeRequestState::Merged,
                selector_from: Some("work/source".into()),
                selector_to: "main".into(),
                ticket_ids: vec!["T".into()],
                created_at: now,
                updated_at: now,
                thread: vec![],
            };
            let approval_id = format!("integration-{}", subject.merge_request_id);
            add_review(
                &mut request,
                &approval_id,
                "digest-one",
                &snapshot,
                ReviewDecision::Approve,
            );
            request
                .thread
                .push(MergeRequestThreadEvent::Merge(MergeEvent {
                    event_id: format!("merge-{}", subject.merge_request_id),
                    sequence: 3,
                    operation_id: format!("operation-{}", subject.merge_request_id),
                    approval_event_id: approval_id,
                    approved_source_ref: subject.subject_ref.clone(),
                    target_ref_before: "target-before".into(),
                    target_ref_after: "target-after".into(),
                    strategy: MergeStrategy::FastForward,
                    resolution: ConflictResolution::None,
                    merged_by: actor(),
                    created_at: now,
                }));
            request
        })
        .collect()
}

#[test]
fn postmerge_latest_approval_attests_the_current_digest_without_replacing_integration() {
    let mut requests = merged_requests();
    let snapshot = subjects();
    assert_eq!(
        requirement_approval(&requests, "digest-two", &snapshot, Some("integration-MR-1")),
        Err(MergeRequestEvidenceError::TicketContentMismatch)
    );
    add_review(
        &mut requests[0],
        "postmerge",
        "digest-two",
        &snapshot,
        ReviewDecision::Approve,
    );

    assert_eq!(
        requests[0].integration_approval().unwrap().event_id,
        "integration-MR-1"
    );
    assert_eq!(
        requirement_approval(&requests, "digest-two", &snapshot, None)
            .unwrap()
            .event_id,
        "postmerge"
    );
    assert_eq!(
        requirement_approval(&requests, "digest-two", &snapshot, Some("postmerge"))
            .unwrap()
            .event_id,
        "postmerge"
    );
    assert_eq!(
        requirement_approval(&requests, "digest-one", &snapshot, Some("integration-MR-1")),
        Err(MergeRequestEvidenceError::ApprovalNotEffective)
    );
    assert_eq!(
        requirement_approval(&requests, "digest-two", &snapshot, Some("unknown")),
        Err(MergeRequestEvidenceError::RequirementApprovalMissing)
    );
}

#[test]
fn later_request_changes_prevents_older_requirement_approval_from_being_effective() {
    let mut requests = merged_requests();
    let snapshot = subjects();
    add_review(
        &mut requests[0],
        "approved",
        "digest-two",
        &snapshot,
        ReviewDecision::Approve,
    );
    add_review(
        &mut requests[0],
        "changes",
        "digest-two",
        &snapshot,
        ReviewDecision::RequestChanges,
    );
    assert_eq!(
        requirement_approval(&requests, "digest-two", &snapshot, None),
        Err(MergeRequestEvidenceError::RequirementApprovalMissing)
    );
    assert_eq!(
        requirement_approval(&requests, "digest-two", &snapshot, Some("approved")),
        Err(MergeRequestEvidenceError::ApprovalNotEffective)
    );
    assert_eq!(
        requirement_approval(&requests, "digest-two", &snapshot, Some("changes")),
        Err(MergeRequestEvidenceError::ApprovalNotApproved)
    );
    assert!(requests[0].integration_approval().is_ok());
}

#[test]
fn changed_digest_or_any_linked_source_invalidates_a_multiple_mr_attestation() {
    let requests = merged_requests();
    let original = subjects();
    assert!(requirement_approval(&requests, "digest-one", &original, None).is_ok());
    assert_eq!(
        requirement_approval(&requests, "digest-two", &original, Some("integration-MR-1")),
        Err(MergeRequestEvidenceError::TicketContentMismatch)
    );
    let mut changed = original.clone();
    changed[1].subject_ref = "changed-source-two".into();
    assert_eq!(
        requirement_approval(&requests, "digest-one", &changed, Some("integration-MR-1")),
        Err(MergeRequestEvidenceError::SourceSnapshotMismatch)
    );
    let mut reversed = original.clone();
    reversed.reverse();
    assert!(requirement_approval(&requests, "digest-one", &reversed, None).is_ok());

    // Even after the caller refreshes the linked request collection, an old
    // attestation cannot authorize completion against a smaller linked set.
    assert_eq!(
        requirement_approval(
            &requests[..1],
            "digest-one",
            &original[..1],
            Some("integration-MR-1")
        ),
        Err(MergeRequestEvidenceError::SourceSnapshotMismatch)
    );
    assert_eq!(
        requirement_approval(&requests, "digest-one", &original[..1], None),
        Err(MergeRequestEvidenceError::SourceSnapshotMismatch)
    );
    assert_eq!(
        requirement_approval(
            &requests,
            "digest-one",
            &[original[0].clone(), original[0].clone()],
            None
        ),
        Err(MergeRequestEvidenceError::SourceSnapshotMismatch)
    );
}

#[test]
fn refreshed_postmerge_approval_can_attest_a_changed_multiple_mr_set_and_digest() {
    let mut requests = merged_requests();
    let mut snapshot = subjects();
    snapshot.pop();
    requests.pop();
    assert_eq!(
        requirement_approval(&requests, "digest-two", &snapshot, None),
        Err(MergeRequestEvidenceError::RequirementApprovalMissing)
    );
    add_review(
        &mut requests[0],
        "refreshed-set",
        "digest-two",
        &snapshot,
        ReviewDecision::Approve,
    );
    assert_eq!(
        requirement_approval(&requests, "digest-two", &snapshot, None)
            .unwrap()
            .event_id,
        "refreshed-set"
    );
    assert_eq!(
        requests[0].integration_approval().unwrap().event_id,
        "integration-MR-1"
    );
}

#[test]
fn requirement_approval_must_bind_its_own_mr_source_and_corresponding_request() {
    let original = subjects();
    for (case, expected) in [
        ("missing", MergeRequestEvidenceError::ReviewRequestMissing),
        ("digest", MergeRequestEvidenceError::ReviewRequestMismatch),
        ("snapshot", MergeRequestEvidenceError::ReviewRequestMismatch),
        ("reviewer", MergeRequestEvidenceError::ReviewRequestMismatch),
        ("source", MergeRequestEvidenceError::ApprovalSourceMismatch),
    ] {
        let mut requests = merged_requests();
        for event in &mut requests[0].thread {
            match event {
                MergeRequestThreadEvent::Review(review) if case == "missing" => {
                    review.request_event_id = "absent-request".into();
                }
                MergeRequestThreadEvent::ReviewRequested(requested) => match case {
                    "digest" => requested.ticket_content_digest = "other-digest".into(),
                    "snapshot" => {
                        requested.ticket_merge_request_subjects.pop();
                    }
                    "reviewer" => requested.reviewer.worker_id = "other-reviewer".into(),
                    "source" => requested.subject_ref = "unbound-source".into(),
                    _ => {}
                },
                MergeRequestThreadEvent::Review(review) if case == "source" => {
                    review.subject_ref = "unbound-source".into();
                }
                _ => {}
            }
        }
        assert_eq!(
            requirement_approval(&requests, "digest-one", &original, Some("integration-MR-1")),
            Err(expected),
            "{case}"
        );
    }
}

#[test]
fn revoked_requirement_approval_is_rejected_and_discovery_uses_another_valid_mr() {
    let mut requests = merged_requests();
    let created_at = requests[0].created_at;
    requests[0]
        .thread
        .push(MergeRequestThreadEvent::ReviewRevoked(ReviewRevokedEvent {
            event_id: "revocation".into(),
            sequence: 4,
            review_event_id: "integration-MR-1".into(),
            subject_ref: "source-one".into(),
            reason: String::new(),
            revoked_by: actor(),
            created_at,
        }));
    assert_eq!(
        requirement_approval(
            &requests,
            "digest-one",
            &subjects(),
            Some("integration-MR-1")
        ),
        Err(MergeRequestEvidenceError::ApprovalRevoked)
    );
    assert_eq!(
        requirement_approval(&requests, "digest-one", &subjects(), None)
            .unwrap()
            .event_id,
        "integration-MR-2"
    );
}

#[test]
fn integration_approval_must_strictly_predate_the_merge_sequence() {
    for approval_sequence in [2, 3, 4] {
        let mut requests = merged_requests();
        for event in &mut requests[0].thread {
            if let MergeRequestThreadEvent::Review(review) = event {
                review.sequence = approval_sequence;
            }
        }
        let result = requests[0].integration_approval();
        if approval_sequence < 3 {
            assert!(result.is_ok());
        } else {
            assert_eq!(
                result,
                Err(MergeRequestEvidenceError::ApprovalTimelineMismatch),
                "approval sequence {approval_sequence}"
            );
        }
    }
}

#[test]
fn blank_saved_source_is_not_valid_integration_evidence_even_when_the_review_matches() {
    for source in ["", " \t\n"] {
        let mut requests = merged_requests();
        for event in &mut requests[0].thread {
            match event {
                MergeRequestThreadEvent::ReviewRequested(requested) => {
                    requested.subject_ref = source.into();
                }
                MergeRequestThreadEvent::Review(review) => {
                    review.subject_ref = source.into();
                }
                MergeRequestThreadEvent::Merge(merge) => {
                    merge.approved_source_ref = source.into();
                }
                _ => {}
            }
        }
        assert_eq!(
            requests[0].integration_approval(),
            Err(MergeRequestEvidenceError::ApprovalSourceMismatch)
        );
    }
}

#[test]
fn evidence_error_codes_are_stable_machine_diagnostics_separate_from_messages() {
    for (error, code) in [
        (MergeRequestEvidenceError::NotMerged, "not_merged"),
        (
            MergeRequestEvidenceError::MergeResultMissing,
            "merge_result_missing",
        ),
        (
            MergeRequestEvidenceError::ApprovalMissing,
            "approval_missing",
        ),
        (
            MergeRequestEvidenceError::ApprovalRevoked,
            "approval_revoked",
        ),
        (
            MergeRequestEvidenceError::ApprovalNotApproved,
            "approval_not_approved",
        ),
        (
            MergeRequestEvidenceError::ApprovalSourceMismatch,
            "approval_source_mismatch",
        ),
        (
            MergeRequestEvidenceError::ApprovalTimelineMismatch,
            "approval_timeline_mismatch",
        ),
        (
            MergeRequestEvidenceError::ReviewRequestMissing,
            "review_request_missing",
        ),
        (
            MergeRequestEvidenceError::ReviewRequestMismatch,
            "review_request_mismatch",
        ),
        (
            MergeRequestEvidenceError::ApprovalNotEffective,
            "approval_not_effective",
        ),
        (
            MergeRequestEvidenceError::TicketContentMismatch,
            "ticket_content_mismatch",
        ),
        (
            MergeRequestEvidenceError::SourceSnapshotMismatch,
            "source_snapshot_mismatch",
        ),
        (
            MergeRequestEvidenceError::RequirementApprovalMissing,
            "requirement_approval_missing",
        ),
    ] {
        assert_eq!(error.code(), code);
        assert_ne!(error.code(), error.as_str());
    }
}
