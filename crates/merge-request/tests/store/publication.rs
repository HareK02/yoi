//! Real SQLite approval validation and confirmed completion. Git transport is
//! exercised by Runtime/Backend connected tests, not by this store suite.
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
fn independent_preparation_never_reserves_a_target() {
    let (dir, _) = fixture();
    let (a, ba) = prepare_independent_mr(dir.path(), "T-a", "MR-a", "refs/heads/develop");
    let (b, bb) = prepare_independent_mr(dir.path(), "T-b", "MR-b", "refs/heads/develop");
    assert!(a.validate_publication(&ba, |_| Ok(())).is_ok());
    assert!(b.validate_publication(&bb, |_| Ok(())).is_ok());
    assert!(a.publications("W", "MR-a").unwrap().is_empty());
    assert!(b.publications("W", "MR-b").unwrap().is_empty());
}

#[test]
fn validation_is_current_and_does_not_persist_dispatch() {
    let (_dir, store) = fixture();
    open(&store);
    let b = binding(&store);
    let digest = store.validate_publication(&b, |_| Ok(())).unwrap();
    assert_eq!(store.validate_publication(&b, |_| Ok(())).unwrap(), digest);
    assert!(
        store
            .validate_publication(&b, |_| Err(MergeRequestError::Unauthorized(
                "revoked".into()
            )))
            .is_err()
    );
    assert!(store.publications("W", "MR").unwrap().is_empty());
    assert!(!store.get_by_id("W", "MR").unwrap().state.is_completed());
}

#[test]
fn confirmed_completion_is_atomic_and_survives_owner_transfer() {
    let (dir, store) = fixture();
    open(&store);
    let b = binding(&store);
    let digest = store.validate_publication(&b, |_| Ok(())).unwrap();
    store
        .set_worker(
            ownership::transfer(&store, "transfer", Some("successor")),
            |_| Ok(()),
        )
        .unwrap();
    let c = Connection::open(dir.path().join("db")).unwrap();
    c.execute("DELETE FROM ticket_current_worker_assignments", [])
        .unwrap();
    let done = store
        .record_confirmed_publication(&b, &digest, at(9))
        .unwrap();
    assert_eq!(done.approved_source_ref, "source");
    assert_eq!(done.target_ref_before, "before");
    assert_eq!(done.target_ref_after, "source");
    assert_eq!(
        store
            .record_confirmed_publication(&b, &digest, at(10))
            .unwrap(),
        done
    );
    assert!(store.get_by_id("W", "MR").unwrap().state.is_completed());
    assert!(store.publications("W", "MR").unwrap().is_empty());
}

#[test]
fn completion_replay_rejects_changed_owner_even_for_the_same_worker() {
    let (_dir, store) = fixture();
    open(&store);
    let mut b = binding(&store);
    let digest = store.validate_publication(&b, |_| Ok(())).unwrap();
    let recorded = store
        .record_confirmed_publication(&b, &digest, at(9))
        .unwrap();
    let owner = store
        .set_worker(
            ownership::transfer(&store, "same-worker-new-owner", Some("coder")),
            |_| Ok(()),
        )
        .unwrap();
    b.owner_operation_id = owner.operation_id;
    assert!(matches!(
        store.record_confirmed_publication(&b, &digest, at(10)),
        Err(MergeRequestError::Conflict(_))
    ));
    assert_eq!(
        store
            .get_by_id("W", "MR")
            .unwrap()
            .thread
            .iter()
            .filter(|e| matches!(e, MergeRequestThreadEvent::Merge(_)))
            .count(),
        1
    );
    assert_ne!(
        recorded.owner_operation_id.as_deref(),
        Some(b.owner_operation_id.as_str())
    );
}

#[test]
fn changed_requirements_or_cancellation_prevent_false_completion() {
    for cancel in [false, true] {
        let (dir, store) = fixture();
        open(&store);
        let b = binding(&store);
        let digest = store.validate_publication(&b, |_| Ok(())).unwrap();
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
        assert!(
            store
                .record_confirmed_publication(&b, &digest, at(9))
                .is_err()
        );
        let problem = store
            .record_publication_problem(
                "W",
                "MR",
                "Git succeeded, but current approval no longer permits completion",
                at(10),
            )
            .unwrap();
        assert!(problem.author.is_none());
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
        assert!(store.validate_publication(&invalid, |_| Ok(())).is_err());
    }
    let c = Connection::open(dir.path().join("db")).unwrap();
    c.execute("UPDATE typed_tickets SET title='new requirements'", [])
        .unwrap();
    assert!(store.validate_publication(&b, |_| Ok(())).is_err());
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
