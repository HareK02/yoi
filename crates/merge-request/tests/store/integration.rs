//! Component coverage uses the real MR store and a temporary SQLite database.
//! Assignment/provider boundaries use the shared fixture; no Git side effects run.
use super::*;

fn submit(token: &str, source: &str, decision: ReviewDecision) -> SubmitMergeRequestReview {
    SubmitMergeRequestReview {
        merge_request_id: "MR".into(),
        ticket_id: "T".into(),
        current_subject_ref: source.into(),
        capability_token: token.into(),
        decision,
        body: "review".into(),
        findings: vec![],
        now: at(4),
    }
}
fn ready(store: &MergeRequestStore, source: &str) -> bool {
    store
        .readiness(ReadinessCheck {
            merge_request_id: "MR".into(),
            ticket_id: "T".into(),
            current_subject_ref: Some(source.into()),
            auth: auth(),
        })
        .unwrap()
        .ready
}
pub(super) fn completion(review: &ReviewEvent) -> CompleteMergeRequest {
    CompleteMergeRequest {
        merge_request_id: "MR".into(),
        ticket_id: "T".into(),
        operation_id: "finish".into(),
        approval_event_id: review.event_id.clone(),
        current_subject_ref: review.subject_ref.clone(),
        target_ref_before: "before".into(),
        target_ref_after: review.subject_ref.clone(),
        strategy: MergeStrategy::FastForward,
        resolution: ConflictResolution::None,
        auth: auth(),
        now: at(8),
    }
}

#[test]
fn phase_tracks_first_valid_approval_not_current_readiness() {
    let (_dir, store) = fixture();
    open(&store);
    assert_eq!(
        store.get("W", "T").unwrap().phase(),
        MergeRequestPhase::Implementation
    );
    request(&store, "one", "changes");
    store
        .submit_review(submit("changes", "one", ReviewDecision::RequestChanges))
        .unwrap();
    assert_eq!(
        store.get("W", "T").unwrap().phase(),
        MergeRequestPhase::Implementation
    );
    let first = approve(&store, "one", "first");
    assert_eq!(
        store.get("W", "T").unwrap().phase(),
        MergeRequestPhase::Integration
    );
    assert!(ready(&store, "one"));
    assert!(!ready(&store, "two"));
    store
        .revoke_review(RevokeMergeRequestReview {
            merge_request_id: "MR".into(),
            ticket_id: "T".into(),
            review_event_id: first.event_id.clone(),
            reason: "recheck".into(),
            auth: mr_auth(&store, "MR", "R"),
            now: at(5),
        })
        .unwrap();
    assert!(!ready(&store, "one"));
    request(&store, "two", "second-changes");
    store
        .submit_review(submit(
            "second-changes",
            "two",
            ReviewDecision::RequestChanges,
        ))
        .unwrap();
    let saved = store.get("W", "T").unwrap();
    assert_eq!(saved.phase(), MergeRequestPhase::Integration);
    assert_eq!(saved.state.started_by(), Some(first.event_id.as_str()));
    let final_review = approve(&store, "three", "final");
    let merged = store.complete(completion(&final_review)).unwrap();
    let saved = store.get("W", "T").unwrap();
    assert_eq!(saved.phase(), MergeRequestPhase::Completed);
    assert_eq!(
        saved.state.started_by().map(str::to_owned),
        Some(first.event_id)
    );
    approve(&store, "three", "post-merge");
    assert_eq!(
        store.get("W", "T").unwrap().merged_result().unwrap(),
        &merged
    );
    assert_eq!(
        store.get("W", "T").unwrap().phase(),
        MergeRequestPhase::Completed
    );
}

#[test]
fn stale_requirements_and_source_cannot_start_integration() {
    for moved_source in [false, true] {
        let (dir, store) = fixture();
        open(&store);
        request(&store, "one", "review");
        if !moved_source {
            Connection::open(dir.path().join("db"))
                .unwrap()
                .execute("UPDATE typed_tickets SET title='Changed'", [])
                .unwrap();
        }
        let result = store.submit_review(submit(
            "review",
            if moved_source { "two" } else { "one" },
            ReviewDecision::Approve,
        ));
        if moved_source {
            assert!(result.is_err());
        } else {
            result.unwrap();
        }
        let saved = store.get("W", "T").unwrap();
        assert_eq!(saved.phase(), MergeRequestPhase::Implementation);
        assert!(saved.state.started_by().is_none());
        assert!(!ready(&store, "one"));
    }
}

#[test]
fn approval_and_phase_roll_back_together_and_duplicate_does_not_reenter() {
    let (dir, store) = fixture();
    open(&store);
    request(&store, "one", "review");
    let c = Connection::open(dir.path().join("db")).unwrap();
    c.execute_batch("CREATE TRIGGER fail_phase BEFORE UPDATE OF state ON merge_requests BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(
        store
            .submit_review(submit("review", "one", ReviewDecision::Approve))
            .is_err()
    );
    assert_eq!(
        store.get("W", "T").unwrap().phase(),
        MergeRequestPhase::Implementation
    );
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM merge_request_thread_events WHERE kind='review'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        c.query_row(
            "SELECT status FROM merge_request_review_grants WHERE capability_token='review'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "issued"
    );
    c.execute_batch("DROP TRIGGER fail_phase").unwrap();
    let review = store
        .submit_review(submit("review", "one", ReviewDecision::Approve))
        .unwrap();
    assert!(
        store
            .submit_review(submit("review", "one", ReviewDecision::Approve))
            .is_err()
    );
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM merge_request_thread_events WHERE kind='review'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        store
            .get("W", "T")
            .unwrap()
            .state
            .started_by()
            .map(str::to_owned),
        Some(review.event_id)
    );
}

#[test]
fn canceled_phase_is_not_success_and_cannot_be_reviewed() {
    let (dir, store) = fixture();
    open(&store);
    approve(&store, "one", "first");
    let c = Connection::open(dir.path().join("db")).unwrap();
    c.execute("UPDATE merge_requests SET state=json_object('kind','canceled','from',json_object('kind','integration','started_by',json_extract(state,'$.started_by')))", [])
        .unwrap();
    let saved = store.get("W", "T").unwrap();
    assert_eq!(saved.phase(), MergeRequestPhase::Canceled);
    assert!(saved.merged_result().is_err());
    assert!(!ready(&store, "one"));
}

#[test]
fn migration_preserves_review_bytes_without_inventing_entry_and_rolls_back() {
    let (dir, store) = fixture();
    open(&store);
    let first = approve(&store, "one", "first");
    store
        .revoke_review(RevokeMergeRequestReview {
            merge_request_id: "MR".into(),
            ticket_id: "T".into(),
            review_event_id: first.event_id.clone(),
            reason: "later".into(),
            auth: mr_auth(&store, "MR", "R"),
            now: at(5),
        })
        .unwrap();
    let c = Connection::open(dir.path().join("db")).unwrap();
    let payload: String = c
        .query_row(
            "SELECT payload_json FROM merge_request_thread_events WHERE event_id=?1",
            [&first.event_id],
            |r| r.get(0),
        )
        .unwrap();
    freeze_v13_schema(&c);
    c.execute_batch("CREATE TRIGGER fail_migration BEFORE UPDATE ON merge_request_schema BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(migrate(&c).is_err());
    assert!(c.prepare("SELECT lifecycle FROM merge_requests").is_err());
    assert_eq!(
        c.query_row("SELECT version FROM merge_request_schema", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        13
    );
    assert_eq!(
        c.query_row("SELECT state FROM merge_requests", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "open"
    );
    assert!(
        c.execute("UPDATE merge_requests SET state='integration'", [])
            .is_err()
    );
    assert_eq!(
        c.query_row(
            "SELECT payload_json FROM merge_request_thread_events WHERE event_id=?1",
            [&first.event_id],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        payload
    );
    assert_eq!(c.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name LIKE 'merge_request_lifecycle_%'", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    c.execute_batch("DROP TRIGGER fail_migration").unwrap();
    migrate(&c).unwrap();
    migrate(&c).unwrap();
    let saved = store.get("W", "T").unwrap();
    assert_eq!(saved.phase(), MergeRequestPhase::Implementation);
    assert!(saved.state.started_by().is_none());
    assert_eq!(
        c.query_row(
            "SELECT payload_json FROM merge_request_thread_events WHERE event_id=?1",
            [&first.event_id],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        payload
    );
    assert!(!ready(&store, "one"));
    let fresh_dir = Connection::open_in_memory().unwrap();
    // Compare actual MR schema objects, not just the version marker.
    let schema = |connection: &Connection| {
        connection.prepare("SELECT type,name,sql FROM sqlite_master WHERE name LIKE 'merge_request%' AND name NOT LIKE 'merge_request_legacy%' ORDER BY type,name").unwrap()
            .query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?))).unwrap().collect::<Result<Vec<_>,_>>().unwrap()
    };
    fresh_dir.execute_batch("CREATE TABLE repositories(workspace_id TEXT,repository_id TEXT,PRIMARY KEY(workspace_id,repository_id));CREATE TABLE typed_tickets(workspace_id TEXT,ticket_id TEXT,PRIMARY KEY(workspace_id,ticket_id));").unwrap();
    migrate(&fresh_dir).unwrap();
    assert_eq!(schema(&c), schema(&fresh_dir));
}

#[test]
fn migration_does_not_infer_an_entry_from_unknown_or_changed_requirements() {
    for legacy in [false, true] {
        let (dir, store) = fixture();
        open(&store);
        approve(&store, "source", "first");
        let c = Connection::open(dir.path().join("db")).unwrap();
        if legacy {
            freeze_v12_review_payloads(&c);
        } else {
            freeze_v13_schema(&c);
            c.execute("UPDATE typed_tickets SET title='Changed'", [])
                .unwrap();
        }
        migrate(&c).unwrap();
        let saved = store.get("W", "T").unwrap();
        assert_eq!(saved.phase(), MergeRequestPhase::Implementation);
        assert!(saved.state.started_by().is_none());
        assert!(!ready(&store, "source"));
    }
}

#[test]
fn concurrent_approvals_keep_one_entry_and_both_review_events() {
    let (dir, store) = fixture();
    open(&store);
    request(&store, "source", "first");
    request(&store, "source", "second");
    let mut threads = vec![];
    for token in ["first", "second"] {
        let second = MergeRequestStore::open(
            dir.path().join("db"),
            Arc::new(Assignments(Arc::new(Mutex::new(CurrentAssignment {
                assignment_id: "A".into(),
                ticket_id: "T".into(),
                runtime_id: "runtime".into(),
                worker_id: "coder".into(),
            })))),
            Arc::new(Repositories),
        )
        .unwrap();
        threads.push(std::thread::spawn(move || {
            second
                .submit_review(submit(token, "source", ReviewDecision::Approve))
                .unwrap()
        }));
    }
    let reviews: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    let first = reviews.iter().min_by_key(|r| r.sequence).unwrap();
    let saved = store.get("W", "T").unwrap();
    assert_eq!(saved.state.started_by(), Some(first.event_id.as_str()));
    assert_eq!(
        saved
            .thread
            .iter()
            .filter(|e| matches!(e, MergeRequestThreadEvent::Review(_)))
            .count(),
        2
    );
}

#[test]
fn migration_cannot_promote_a_stale_approval_after_requirements_return() {
    let (dir, store) = fixture();
    open(&store);
    request(&store, "source", "old");
    let c = Connection::open(dir.path().join("db")).unwrap();
    c.execute("UPDATE typed_tickets SET title='Changed during review'", [])
        .unwrap();
    let old = store
        .submit_review(submit("old", "source", ReviewDecision::Approve))
        .unwrap();
    assert_eq!(
        store.get("W", "T").unwrap().phase(),
        MergeRequestPhase::Implementation
    );
    c.execute("UPDATE typed_tickets SET title='Title'", [])
        .unwrap();
    freeze_v13_schema(&c);
    migrate(&c).unwrap();
    assert_eq!(
        store.get("W", "T").unwrap().phase(),
        MergeRequestPhase::Implementation
    );
    assert!(!ready(&store, "source"));
    assert!(store.complete(completion(&old)).is_err());
    store
        .set_worker(
            super::ownership::transfer(&store, "legacy-owner", Some("coder")),
            |_| Ok(()),
        )
        .unwrap();
    let fresh = approve(&store, "source", "fresh");
    assert_eq!(
        store
            .get("W", "T")
            .unwrap()
            .state
            .started_by()
            .map(str::to_owned),
        Some(fresh.event_id.clone())
    );
    assert!(ready(&store, "source"));
    store.complete(completion(&fresh)).unwrap();
    assert_eq!(
        store.get("W", "T").unwrap().phase(),
        MergeRequestPhase::Completed
    );
}
