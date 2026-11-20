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
