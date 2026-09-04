use chrono::{TimeZone, Utc};
use merge_request::*;
use rusqlite::Connection;
use std::sync::{Arc, Mutex};
#[derive(Clone)]
struct Assignments(Arc<Mutex<CurrentAssignment>>);
impl AssignmentSource for Assignments {
    fn current_assignment(&self, _: &str, _: &str) -> Result<Option<CurrentAssignment>, String> {
        Ok(Some(self.0.lock().unwrap().clone()))
    }
}
struct Repositories;
impl RepositorySource for Repositories {
    fn repository_belongs_to_workspace(&self, w: &str, r: &str) -> Result<bool, String> {
        Ok(w == "W" && r == "R")
    }
}
fn at(s: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 26, 12, 0, s)
        .single()
        .unwrap()
}
fn auth() -> MergeRequestAuth {
    MergeRequestAuth {
        workspace_id: "W".into(),
        repository_id: "R".into(),
        runtime_id: "runtime".into(),
        worker_id: "coder".into(),
        assignment_id: "A".into(),
    }
}
fn fixture() -> (tempfile::TempDir, MergeRequestStore) {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let c = Connection::open(&p).unwrap();
    c.execute_batch("CREATE TABLE workspaces(workspace_id TEXT PRIMARY KEY);CREATE TABLE repositories(workspace_id TEXT,repository_id TEXT,PRIMARY KEY(workspace_id,repository_id));CREATE TABLE ticket_current_worker_assignments(workspace_id TEXT,ticket_id TEXT,assignment_id TEXT,runtime_id TEXT,worker_id TEXT,updated_at TEXT,PRIMARY KEY(workspace_id,ticket_id));CREATE TABLE typed_tickets(workspace_id TEXT,ticket_id TEXT,workflow_state TEXT,workflow_state_explicit INTEGER,updated_at TEXT,PRIMARY KEY(workspace_id,ticket_id));CREATE TABLE typed_ticket_events(workspace_id TEXT,ticket_id TEXT,event_index INTEGER,kind TEXT,author TEXT,at TEXT,from_state TEXT,to_state TEXT,heading TEXT,body TEXT,PRIMARY KEY(workspace_id,ticket_id,event_index));CREATE TABLE typed_ticket_event_attributes(workspace_id TEXT,ticket_id TEXT,event_index INTEGER,key TEXT,value TEXT,PRIMARY KEY(workspace_id,ticket_id,event_index,key));INSERT INTO workspaces VALUES('W');INSERT INTO repositories VALUES('W','R');INSERT INTO ticket_current_worker_assignments VALUES('W','T','A','runtime','coder','t');INSERT INTO typed_tickets VALUES('W','T','inprogress',1,'t');").unwrap();
    drop(c);
    let a = Assignments(Arc::new(Mutex::new(CurrentAssignment {
        assignment_id: "A".into(),
        ticket_id: "T".into(),
        runtime_id: "runtime".into(),
        worker_id: "coder".into(),
    })));
    let s = MergeRequestStore::open(&p, Arc::new(a), Arc::new(Repositories)).unwrap();
    (d, s)
}
fn open(s: &MergeRequestStore) {
    s.open_merge_request(OpenMergeRequest {
        merge_request_id: "MR".into(),
        ticket_id: "T".into(),
        repository_id: "R".into(),
        selector_from: "work/t".into(),
        selector_to: "develop".into(),
        summary: "summary".into(),
        auth: auth(),
        now: at(1),
    })
    .unwrap();
}
fn request(s: &MergeRequestStore, subject: &str, token: &str) -> ReviewRequestedEvent {
    s.register_reviewer_child_session(RegisterReviewerChildSession {
        workspace_id: "W".into(),
        parent_runtime_id: "runtime".into(),
        parent_worker_id: "coder".into(),
        child_session_id: format!("child-{token}"),
        reviewer_profile: "builtin:reviewer".into(),
        now: at(2),
    })
    .unwrap();
    s.request_review(RequestMergeRequestReview {
        ticket_id: "T".into(),
        subject_ref: subject.into(),
        child_session_id: format!("child-{token}"),
        capability_token: token.into(),
        auth: auth(),
        now: at(3),
    })
    .unwrap()
    .request_event
}
fn approve(s: &MergeRequestStore, subject: &str, token: &str) -> ReviewEvent {
    request(s, subject, token);
    s.submit_review(SubmitMergeRequestReview {
        ticket_id: "T".into(),
        current_subject_ref: subject.into(),
        capability_token: token.into(),
        decision: ReviewDecision::Approve,
        body: "approved".into(),
        findings: vec![],
        now: at(4),
    })
    .unwrap()
}
#[test]
fn review_submission_authorization_rejects_invalid_grants_before_side_effects() {
    let (_d, store) = fixture();
    open(&store);
    request(&store, "published-source", "valid-token");

    let invalid = store
        .authorize_review_submission("T", "invalid-token")
        .unwrap_err();
    assert!(matches!(invalid, MergeRequestError::Unauthorized(_)));
    let authorized = store
        .authorize_review_submission("T", "valid-token")
        .unwrap();
    assert_eq!(authorized.workspace_id, "W");
    assert_eq!(authorized.subject_ref, "published-source");
}

#[test]
fn selectors_thread_and_completion_have_no_revision_or_commit_api() {
    let (d, s) = fixture();
    open(&s);
    let review = approve(&s, "opaque-source-ref", "token");
    let ready = s
        .readiness(ReadinessCheck {
            ticket_id: "T".into(),
            current_subject_ref: Some("opaque-source-ref".into()),
            auth: auth(),
        })
        .unwrap();
    assert!(ready.ready);
    let merged = s
        .complete(CompleteMergeRequest {
            ticket_id: "T".into(),
            operation_id: "op".into(),
            approval_event_id: review.event_id,
            current_subject_ref: "opaque-source-ref".into(),
            target_ref_before: "old-target-ref".into(),
            target_ref_after: "new-target-ref".into(),
            strategy: MergeStrategy::FastForward,
            resolution: ConflictResolution::None,
            auth: auth(),
            now: at(5),
        })
        .unwrap();
    assert_eq!(merged.approved_source_ref, "opaque-source-ref");
    let mr = s.get("W", "T").unwrap();
    assert_eq!(mr.selector_from.as_deref(), Some("work/t"));
    assert_eq!(mr.state, MergeRequestState::Merged);
    let current_assignment: bool = Connection::open(d.path().join("db"))
        .unwrap()
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM ticket_current_worker_assignments
                 WHERE workspace_id='W' AND ticket_id='T'
             )",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!current_assignment);
    let replayed = s
        .complete(CompleteMergeRequest {
            ticket_id: "T".into(),
            operation_id: "op".into(),
            approval_event_id: merged.approval_event_id.clone(),
            current_subject_ref: merged.approved_source_ref.clone(),
            target_ref_before: merged.target_ref_before.clone(),
            target_ref_after: merged.target_ref_after.clone(),
            strategy: merged.strategy,
            resolution: merged.resolution,
            auth: auth(),
            now: at(6),
        })
        .unwrap();
    assert_eq!(replayed, merged);
    let json = serde_json::to_string(&mr).unwrap();
    for banned in [
        "revision_id",
        "attempt_id",
        "base_commit",
        "head_commit",
        "source_commit",
        "result_commit",
        "current_revision",
    ] {
        assert!(!json.contains(banned), "{banned} in {json}")
    }
}
#[test]
fn source_move_cancels_submission_and_old_approval_is_reusable_when_source_returns() {
    let (_d, s) = fixture();
    open(&s);
    let approved = approve(&s, "source-a", "one");
    request(&s, "source-b", "two");
    assert!(
        s.submit_review(SubmitMergeRequestReview {
            ticket_id: "T".into(),
            current_subject_ref: "source-c".into(),
            capability_token: "two".into(),
            decision: ReviewDecision::Approve,
            body: "stale".into(),
            findings: vec![],
            now: at(6)
        })
        .is_err()
    );
    let mr = s.get("W", "T").unwrap();
    let cancellation = mr.thread.iter().find_map(|event| match event {
        MergeRequestThreadEvent::ReviewCancelled(value) => Some(value),
        _ => None,
    });
    assert!(
        cancellation
            .as_ref()
            .is_some_and(|value| value.reason.contains("selector_from moved")
                && value.reason.contains("fresh review"))
    );
    assert_eq!(
        mr.effective_review("source-a").map(|r| &r.event_id),
        Some(&approved.event_id)
    );
}
#[test]
fn same_selector_source_advancement_requires_fresh_review_and_preserves_target_only_approval() {
    let (_d, s) = fixture();
    open(&s);
    let first = approve(&s, "source-1", "one");

    let stale = s
        .readiness(ReadinessCheck {
            ticket_id: "T".into(),
            current_subject_ref: Some("source-2".into()),
            auth: auth(),
        })
        .unwrap();
    assert!(!stale.ready);
    assert!(stale.review.is_none());
    assert!(stale.blockers.iter().any(|blocker| {
        blocker.contains("selector_from moved from reviewed/requested subject source-1")
            && blocker.contains("current subject source-2")
            && blocker.contains("fresh review")
    }));
    assert_eq!(
        s.get("W", "T")
            .unwrap()
            .effective_review("source-1")
            .map(|review| &review.event_id),
        Some(&first.event_id)
    );

    let second = approve(&s, "source-2", "two");
    let ready = s
        .readiness(ReadinessCheck {
            ticket_id: "T".into(),
            current_subject_ref: Some("source-2".into()),
            auth: auth(),
        })
        .unwrap();
    assert!(ready.ready);
    assert_eq!(
        ready.review.as_ref().map(|review| &review.event_id),
        Some(&second.event_id)
    );

    // The target can move from target-1 to target-2 without changing selector_from
    // or invalidating the exact-source approval. Completion consumes refreshed
    // integration evidence for the current target pair.
    let merged = s
        .complete(CompleteMergeRequest {
            operation_id: "target-moved".into(),
            ticket_id: "T".into(),
            current_subject_ref: "source-2".into(),
            target_ref_before: "target-2".into(),
            target_ref_after: "integrated-target-2".into(),
            approval_event_id: second.event_id,
            strategy: MergeStrategy::FastForward,
            resolution: ConflictResolution::None,
            auth: auth(),
            now: at(5),
        })
        .unwrap();
    assert_eq!(merged.approved_source_ref, "source-2");
    assert_eq!(merged.target_ref_before, "target-2");
    assert_eq!(merged.target_ref_after, "integrated-target-2");
}

#[test]
fn review_revocation_invalidates_readiness() {
    let (_d, s) = fixture();
    open(&s);
    let review = approve(&s, "source", "one");
    s.revoke_review(RevokeMergeRequestReview {
        ticket_id: "T".into(),
        review_event_id: review.event_id,
        reason: "bad evidence".into(),
        auth: auth(),
        now: at(7),
    })
    .unwrap();
    let r = s
        .readiness(ReadinessCheck {
            ticket_id: "T".into(),
            current_subject_ref: Some("source".into()),
            auth: auth(),
        })
        .unwrap();
    assert!(!r.ready);
}

#[test]
fn fresh_schema_uses_version_12_and_reopens_as_current() {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(
        "CREATE TABLE repositories(workspace_id TEXT,repository_id TEXT,PRIMARY KEY(workspace_id,repository_id));CREATE TABLE typed_tickets(workspace_id TEXT,ticket_id TEXT,PRIMARY KEY(workspace_id,ticket_id));",
    )
    .unwrap();

    merge_request::migrate(&c).unwrap();
    assert_eq!(
        c.query_row("SELECT version FROM merge_request_schema", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        12
    );
    merge_request::migrate(&c).unwrap();
}

#[test]
fn current_schema_validation_rejects_missing_tables() {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(
        "CREATE TABLE repositories(workspace_id TEXT,repository_id TEXT,PRIMARY KEY(workspace_id,repository_id));CREATE TABLE typed_tickets(workspace_id TEXT,ticket_id TEXT,PRIMARY KEY(workspace_id,ticket_id));",
    )
    .unwrap();
    merge_request::migrate(&c).unwrap();
    c.execute_batch("DROP TABLE merge_request_review_grants;")
        .unwrap();

    let error = merge_request::migrate(&c).unwrap_err();
    assert!(matches!(
        error,
        MergeRequestError::Corrupt(message)
            if message == "missing `merge_request_review_grants`"
    ));
}

#[test]
fn authority_reads_full_thread_while_public_pages_remain_bounded() {
    let (_d, store) = fixture();
    open(&store);
    for index in 0..55 {
        approve(&store, "same-subject", &format!("token-{index}"));
    }
    let mr = store.get("W", "T").unwrap();
    assert!(mr.thread.len() > 100);
    assert_eq!(
        mr.effective_review("same-subject").unwrap().decision,
        ReviewDecision::Approve
    );
    assert_eq!(store.thread_page("W", "T", None, 20).unwrap().len(), 20);
    assert_eq!(
        store.thread_page("W", "T", Some(100), 20).unwrap().len(),
        11
    );
}

#[test]
fn completion_rejects_superseded_approval_for_same_subject() {
    let (_d, store) = fixture();
    open(&store);
    let old_approval = approve(&store, "subject", "approval");
    request(&store, "subject", "changes");
    store
        .submit_review(SubmitMergeRequestReview {
            ticket_id: "T".into(),
            current_subject_ref: "subject".into(),
            capability_token: "changes".into(),
            decision: ReviewDecision::RequestChanges,
            body: "changes required".into(),
            findings: vec![],
            now: at(5),
        })
        .unwrap();
    let result = store.complete(CompleteMergeRequest {
        ticket_id: "T".into(),
        operation_id: "op".into(),
        approval_event_id: old_approval.event_id,
        current_subject_ref: "subject".into(),
        target_ref_before: "before".into(),
        target_ref_after: "after".into(),
        strategy: MergeStrategy::FastForward,
        resolution: ConflictResolution::None,
        auth: auth(),
        now: at(6),
    });
    assert!(matches!(result, Err(MergeRequestError::NotReady(_))));
}

#[test]
fn completion_cancels_outstanding_grants_and_late_submit_fails() {
    let (_d, store) = fixture();
    open(&store);
    let approval = approve(&store, "subject", "approval");
    request(&store, "other-subject", "pending");
    store
        .complete(CompleteMergeRequest {
            ticket_id: "T".into(),
            operation_id: "op".into(),
            approval_event_id: approval.event_id,
            current_subject_ref: "subject".into(),
            target_ref_before: "before".into(),
            target_ref_after: "after".into(),
            strategy: MergeStrategy::FastForward,
            resolution: ConflictResolution::None,
            auth: auth(),
            now: at(6),
        })
        .unwrap();
    let late = store.submit_review(SubmitMergeRequestReview {
        ticket_id: "T".into(),
        current_subject_ref: "other-subject".into(),
        capability_token: "pending".into(),
        decision: ReviewDecision::Approve,
        body: "too late".into(),
        findings: vec![],
        now: at(7),
    });
    assert!(matches!(late, Err(MergeRequestError::Unauthorized(_))));
    let mr = store.get("W", "T").unwrap();
    assert!(mr.thread.iter().any(|event| matches!(event,
        MergeRequestThreadEvent::ReviewCancelled(value)
            if value.reason.contains("completed before review submission"))));
}

#[test]
fn selector_repair_requires_and_accepts_an_approved_resolved_subject() {
    let (dir, store) = fixture();
    open(&store);
    approve(&store, "approved-subject", "approval");
    Connection::open(dir.path().join("db")).unwrap()
        .execute("UPDATE merge_requests SET selector_from=NULL WHERE workspace_id='W' AND merge_request_id='MR'", [])
        .unwrap();
    let repaired = store
        .repair_selector_from(RepairSelectorFrom {
            workspace_id: "W".into(),
            ticket_id: "T".into(),
            selector_from: "restored-work".into(),
            resolved_subject_ref: "approved-subject".into(),
            repaired_by: WorkerIdentity {
                runtime_id: "browser".into(),
                worker_id: "user".into(),
            },
            reason: "confirmed migrated source".into(),
            now: at(8),
        })
        .unwrap();
    assert_eq!(repaired.selector_from.as_deref(), Some("restored-work"));
}

#[test]
fn selector_repair_rejects_unapproved_resolved_subject() {
    let (dir, store) = fixture();
    open(&store);
    approve(&store, "approved-subject", "approval");
    Connection::open(dir.path().join("db")).unwrap()
        .execute("UPDATE merge_requests SET selector_from=NULL WHERE workspace_id='W' AND merge_request_id='MR'", [])
        .unwrap();
    let result = store.repair_selector_from(RepairSelectorFrom {
        workspace_id: "W".into(),
        ticket_id: "T".into(),
        selector_from: "wrong-work".into(),
        resolved_subject_ref: "different-subject".into(),
        repaired_by: WorkerIdentity {
            runtime_id: "browser".into(),
            worker_id: "user".into(),
        },
        reason: "wrong candidate".into(),
        now: at(8),
    });
    assert!(matches!(result, Err(MergeRequestError::NotReady(_))));
}

#[test]
fn first_class_list_and_detail_are_workspace_scoped_and_cursor_bounded() {
    let (dir, store) = fixture();
    open(&store);
    Connection::open(dir.path().join("db"))
        .unwrap()
        .execute(
            "UPDATE merge_requests SET state='closed' WHERE workspace_id='W' AND merge_request_id='MR'",
            [],
        )
        .unwrap();
    store
        .open_merge_request(OpenMergeRequest {
            merge_request_id: "MR-2".into(),
            ticket_id: "T".into(),
            repository_id: "R".into(),
            selector_from: "work/t-2".into(),
            selector_to: "develop".into(),
            summary: "second".into(),
            auth: auth(),
            now: at(8),
        })
        .unwrap();

    let first = store
        .list(
            "W",
            &MergeRequestListQuery {
                ticket_id: Some("T".into()),
                limit: 1,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(first.items[0].merge_request_id, "MR-2");
    assert_eq!(first.next_cursor.as_deref(), Some("MR-2"));

    let second = store
        .list(
            "W",
            &MergeRequestListQuery {
                ticket_id: Some("T".into()),
                cursor: first.next_cursor,
                limit: 1,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(second.items[0].merge_request_id, "MR");
    assert!(second.next_cursor.is_none());

    let closed = store
        .list(
            "W",
            &MergeRequestListQuery {
                state: Some(MergeRequestState::Closed),
                limit: 10,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(closed.items.len(), 1);
    assert_eq!(store.get_by_id("W", "MR").unwrap().merge_request_id, "MR");
    assert!(matches!(
        store.get_by_id("other", "MR"),
        Err(MergeRequestError::NotFound)
    ));
}

#[test]
fn transactional_completion_rejects_assignment_changed_in_control_plane_db() {
    let (dir, store) = fixture();
    open(&store);
    let approval = approve(&store, "subject", "approval");
    Connection::open(dir.path().join("db")).unwrap()
        .execute("UPDATE ticket_current_worker_assignments SET assignment_id='B' WHERE workspace_id='W' AND ticket_id='T'", [])
        .unwrap();
    let result = store.complete(CompleteMergeRequest {
        ticket_id: "T".into(),
        operation_id: "op".into(),
        approval_event_id: approval.event_id,
        current_subject_ref: "subject".into(),
        target_ref_before: "before".into(),
        target_ref_after: "after".into(),
        strategy: MergeStrategy::FastForward,
        resolution: ConflictResolution::None,
        auth: auth(),
        now: at(9),
    });
    assert!(matches!(result, Err(MergeRequestError::Unauthorized(_))));
    let state: String = Connection::open(dir.path().join("db"))
        .unwrap()
        .query_row(
            "SELECT workflow_state FROM typed_tickets WHERE workspace_id='W' AND ticket_id='T'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "inprogress");
}
