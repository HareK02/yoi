//! Real SQLite publication claims/results, with provider receipts injected only
//! at the trusted evidence boundary. These do not exercise Git transport.
use super::*;

fn binding(store: &MergeRequestStore) -> PublicationBinding {
    let review = approve(store, "source", "publication-review");
    PublicationBinding {
        workspace_id: "W".into(),
        merge_request_id: "MR".into(),
        operation_id: "publication".into(),
        repository_id: "R".into(),
        repository_source_fingerprint: "fingerprint".into(),
        alias: "main".into(),
        target: "refs/heads/develop".into(),
        source: "source".into(),
        approval_event_id: review.event_id,
        owner_operation_id: store
            .current_worker("W", "MR")
            .unwrap()
            .unwrap()
            .operation_id,
        worker: WorkerIdentity {
            runtime_id: "runtime".into(),
            worker_id: "coder".into(),
        },
        working_directory_id: "workdir".into(),
        connection_id: "connection".into(),
        target_before: "before".into(),
    }
}
fn prepare_independent_mr(
    dir: &std::path::Path,
    ticket: &str,
    mr: &str,
    target: &str,
) -> (MergeRequestStore, PublicationBinding) {
    let path = dir.join("db");
    let connection = Connection::open(&path).unwrap();
    connection.execute("INSERT INTO typed_tickets SELECT workspace_id,?1,workflow_state,workflow_state_explicit,updated_at,title,body FROM typed_tickets WHERE ticket_id='T'", [ticket]).unwrap();
    connection.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,kind,body,event_ref) VALUES('W',?1,-1,'comment','Requirements',?2)", [ticket, &format!("{ticket}:-1")]).unwrap();
    connection.execute("INSERT INTO ticket_current_worker_assignments SELECT workspace_id,?1,assignment_id,runtime_id,worker_id,updated_at FROM ticket_current_worker_assignments WHERE ticket_id='T'", [ticket]).unwrap();
    let store = MergeRequestStore::open(
        path,
        Arc::new(Assignments(Arc::new(Mutex::new(CurrentAssignment {
            assignment_id: "A".into(),
            ticket_id: ticket.into(),
            runtime_id: "runtime".into(),
            worker_id: "coder".into(),
        })))),
        Arc::new(Repositories),
    )
    .unwrap();
    store
        .open_merge_request(OpenMergeRequest {
            merge_request_id: mr.into(),
            ticket_id: ticket.into(),
            repository_id: "R".into(),
            selector_from: format!("work/{mr}"),
            selector_to: target.into(),
            summary: String::new(),
            auth: auth(),
            now: at(0),
        })
        .unwrap();
    let token = format!("review-{mr}");
    store
        .register_reviewer_child_session(RegisterReviewerChildSession {
            workspace_id: "W".into(),
            parent_runtime_id: "runtime".into(),
            parent_worker_id: "coder".into(),
            child_session_id: token.clone(),
            reviewer_profile: "builtin:reviewer".into(),
            now: at(2),
        })
        .unwrap();
    let source = format!("source-{mr}");
    store
        .request_review(RequestMergeRequestReview {
            merge_request_id: mr.into(),
            ticket_id: ticket.into(),
            ticket_content_digest: ticket::sqlite_ticket_content_digest(&connection, "W", ticket)
                .unwrap()
                .unwrap(),
            ticket_merge_request_subjects: vec![MergeRequestReviewSubject {
                merge_request_id: mr.into(),
                subject_ref: source.clone(),
            }],
            subject_ref: source.clone(),
            child_session_id: token.clone(),
            capability_token: token.clone(),
            auth: mr_auth(&store, mr, "R"),
            now: at(3),
        })
        .unwrap();
    let review = store
        .submit_review(SubmitMergeRequestReview {
            merge_request_id: mr.into(),
            ticket_id: ticket.into(),
            current_subject_ref: source.clone(),
            capability_token: token,
            decision: ReviewDecision::Approve,
            body: "independent candidate prepared".into(),
            findings: vec![],
            now: at(4),
        })
        .unwrap();
    let binding = PublicationBinding {
        workspace_id: "W".into(),
        merge_request_id: mr.into(),
        operation_id: format!("publish-{mr}"),
        repository_id: "R".into(),
        repository_source_fingerprint: "fingerprint".into(),
        alias: "main".into(),
        target: target.into(),
        source,
        approval_event_id: review.event_id,
        owner_operation_id: store.current_worker("W", mr).unwrap().unwrap().operation_id,
        worker: WorkerIdentity {
            runtime_id: "runtime".into(),
            worker_id: "coder".into(),
        },
        working_directory_id: format!("workdir-{mr}"),
        connection_id: format!("connection-{mr}"),
        target_before: "before".into(),
    };
    (store, binding)
}

#[test]
fn independent_preparation_is_not_serialized_but_same_target_publication_is() {
    let (dir, _) = fixture();
    let (a, ba) = prepare_independent_mr(dir.path(), "T-a", "MR-a", "refs/heads/develop");
    let (b, bb) = prepare_independent_mr(dir.path(), "T-b", "MR-b", "refs/heads/develop");
    let (c, bc) = prepare_independent_mr(dir.path(), "T-c", "MR-c", "refs/heads/release");
    assert!(matches!(
        a.get_by_id("W", "MR-a").unwrap().state,
        MergeRequestState::Integration { .. }
    ));
    assert!(matches!(
        b.get_by_id("W", "MR-b").unwrap().state,
        MergeRequestState::Integration { .. }
    ));
    let barrier = std::sync::Barrier::new(2);
    let (ra, rb) = std::thread::scope(|scope| {
        let left = scope.spawn(|| {
            barrier.wait();
            a.claim_publication(ba.clone(), at(6), |_| Ok(()))
        });
        let right = scope.spawn(|| {
            barrier.wait();
            b.claim_publication(bb.clone(), at(6), |_| Ok(()))
        });
        (left.join().unwrap(), right.join().unwrap())
    });
    assert_ne!(
        ra.is_ok(),
        rb.is_ok(),
        "same target admits exactly one operation: {ra:?} {rb:?}"
    );
    assert!(
        c.claim_publication(bc, at(6), |_| Ok(())).unwrap().1,
        "different target proceeds while the first is unconfirmed"
    );
    let (winner, loser) = if ra.is_ok() { (&ba, &bb) } else { (&bb, &ba) };
    assert!(
        a.claim_publication(loser.clone(), at(7), |_| Ok(()))
            .is_err(),
        "unknown outcome must not release target"
    );
    a.record_publication_outcome(
        winner,
        PublicationOutcome::NotPublished {
            reason: "confirmed pre-send failure".into(),
        },
    )
    .unwrap();
    assert!(
        a.claim_publication(loser.clone(), at(8), |_| Ok(()))
            .unwrap()
            .1
    );
}

fn published() -> PublicationOutcome {
    PublicationOutcome::Published {
        target_after: "source".into(),
        observed_at: at(8).to_rfc3339(),
    }
}

#[test]
fn dispatch_claim_is_exact_and_unknown_result_never_admits_a_second_operation() {
    let (_dir, store) = fixture();
    open(&store);
    let b = binding(&store);
    let (saved, dispatch) = store
        .claim_publication(b.clone(), at(6), |_| Ok(()))
        .unwrap();
    assert!(dispatch);
    assert_eq!(
        store
            .claim_publication(b.clone(), at(7), |_| Ok(()))
            .unwrap(),
        (saved, false)
    );
    let mut changed = b.clone();
    changed.connection_id = "rebound".into();
    assert!(store.claim_publication(changed, at(7), |_| Ok(())).is_err());
    let mut duplicate = b.clone();
    duplicate.operation_id = "second-push".into();
    assert!(
        store
            .claim_publication(duplicate, at(7), |_| Ok(()))
            .is_err()
    );
    assert!(
        store
            .complete_publication("W", "publication", at(8))
            .is_err()
    );
    assert!(
        store
            .claim_publication(b, at(7), |_| Err(MergeRequestError::Unauthorized(
                "revoked".into()
            )))
            .is_err()
    );
}

#[test]
fn provider_result_survives_owner_transfer_and_completion_never_reobserves_moved_target() {
    let (dir, store) = fixture();
    open(&store);
    let b = binding(&store);
    store
        .claim_publication(b.clone(), at(6), |_| Ok(()))
        .unwrap();
    store
        .set_worker(
            ownership::transfer(&store, "transfer", Some("successor")),
            |_| Ok(()),
        )
        .unwrap();
    let c = Connection::open(dir.path().join("db")).unwrap();
    c.execute("DELETE FROM ticket_current_worker_assignments", [])
        .unwrap();
    let receipt = store.record_publication_outcome(&b, published()).unwrap();
    assert_eq!(
        store.record_publication_outcome(&b, published()).unwrap(),
        receipt
    );
    assert!(
        store
            .record_publication_outcome(
                &b,
                PublicationOutcome::NotPublished {
                    reason: "contradiction".into()
                }
            )
            .is_err()
    );
    let done = store
        .complete_publication("W", "publication", at(9))
        .unwrap();
    assert_eq!(done.approved_source_ref, "source");
    assert_eq!(done.target_ref_before, "before");
    assert_eq!(
        store
            .complete_publication("W", "publication", at(10))
            .unwrap(),
        done
    );
    assert!(store.get_by_id("W", "MR").unwrap().state.is_completed());
    assert_eq!(
        store.publication("W", "publication").unwrap().unwrap(),
        receipt
    );
    assert!(
        c.execute("UPDATE merge_request_publications SET confirmed=0", [])
            .is_err()
    );
    assert!(
        c.execute("DELETE FROM merge_request_publications", [])
            .is_err()
    );
}

#[test]
fn canceled_or_rescoped_mr_retains_sent_publication_without_false_completion() {
    for cancel in [false, true] {
        let (dir, store) = fixture();
        open(&store);
        let b = binding(&store);
        store
            .claim_publication(b.clone(), at(6), |_| Ok(()))
            .unwrap();
        let c = Connection::open(dir.path().join("db")).unwrap();
        if cancel {
            let mr = store.get_by_id("W", "MR").unwrap();
            let state = MergeRequestState::Canceled {
                from: MergeRequestCancellationOrigin::Integration {
                    started_by: mr.state.started_by().unwrap().into(),
                },
            };
            c.execute(
                "UPDATE merge_requests SET state=?1",
                [serde_json::to_string(&state).unwrap()],
            )
            .unwrap();
        } else {
            c.execute("UPDATE typed_tickets SET title='changed requirements'", [])
                .unwrap();
        }
        store.record_publication_outcome(&b, published()).unwrap();
        assert!(
            store
                .complete_publication("W", "publication", at(9))
                .is_err()
        );
        assert_eq!(
            store
                .publication("W", "publication")
                .unwrap()
                .unwrap()
                .outcome,
            published()
        );
        assert!(!store.get_by_id("W", "MR").unwrap().state.is_completed());
    }
}

#[test]
fn invalid_binding_stale_owner_and_requirements_are_rejected_before_dispatch() {
    let (dir, store) = fixture();
    open(&store);
    let b = binding(&store);
    for invalid in [
        PublicationBinding {
            source: "other".into(),
            ..b.clone()
        },
        PublicationBinding {
            target: "refs/heads/other".into(),
            ..b.clone()
        },
        PublicationBinding {
            repository_id: "R2".into(),
            ..b.clone()
        },
        PublicationBinding {
            owner_operation_id: "old".into(),
            ..b.clone()
        },
    ] {
        assert!(store.claim_publication(invalid, at(6), |_| Ok(())).is_err());
    }
    let c = Connection::open(dir.path().join("db")).unwrap();
    c.execute("UPDATE typed_tickets SET title='new requirements'", [])
        .unwrap();
    assert!(store.claim_publication(b, at(6), |_| Ok(())).is_err());
    assert!(store.publications("W", "MR").unwrap().is_empty());
}

#[test]
fn publication_schema_migration_is_atomic_and_preserves_ownership() {
    let (dir, store) = fixture();
    open(&store);
    let before = store.get_by_id("W", "MR").unwrap();
    let c = Connection::open(dir.path().join("db")).unwrap();
    c.execute_batch("DROP TABLE merge_request_publications; UPDATE merge_request_schema SET version=15; CREATE TRIGGER reject_publication_schema BEFORE UPDATE ON merge_request_schema BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(merge_request::migrate(&c).is_err());
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name='merge_request_publications'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    c.execute_batch("DROP TRIGGER reject_publication_schema")
        .unwrap();
    merge_request::migrate(&c).unwrap();
    assert_eq!(store.get_by_id("W", "MR").unwrap(), before);
    assert!(store.publications("W", "MR").unwrap().is_empty());
}
