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
        Ok(w == "W" && matches!(r, "R" | "R2"))
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
fn auth_repo(repository_id: &str) -> MergeRequestAuth {
    MergeRequestAuth {
        repository_id: repository_id.into(),
        ..auth()
    }
}
fn content_digest(title: &str) -> String {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("CREATE TABLE typed_tickets(workspace_id TEXT,ticket_id TEXT,title TEXT,body TEXT); CREATE TABLE typed_ticket_targets(workspace_id TEXT,ticket_id TEXT,ordinal INTEGER,repository_key TEXT,ref_selector TEXT,access TEXT);").unwrap();
    connection
        .execute(
            "INSERT INTO typed_tickets VALUES('W','T',?1,'Requirements')",
            [title],
        )
        .unwrap();
    ticket::sqlite_ticket_content_digest(&connection, "W", "T")
        .unwrap()
        .unwrap()
}
fn fixture() -> (tempfile::TempDir, MergeRequestStore) {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let c = Connection::open(&p).unwrap();
    c.execute_batch("CREATE TABLE workspaces(workspace_id TEXT PRIMARY KEY);CREATE TABLE repositories(workspace_id TEXT,repository_id TEXT,PRIMARY KEY(workspace_id,repository_id));CREATE TABLE ticket_current_worker_assignments(workspace_id TEXT,ticket_id TEXT,assignment_id TEXT,runtime_id TEXT,worker_id TEXT,updated_at TEXT,PRIMARY KEY(workspace_id,ticket_id));CREATE TABLE typed_tickets(workspace_id TEXT,ticket_id TEXT,workflow_state TEXT,workflow_state_explicit INTEGER,updated_at TEXT,title TEXT NOT NULL,body TEXT NOT NULL,PRIMARY KEY(workspace_id,ticket_id));CREATE TABLE typed_ticket_targets(workspace_id TEXT,ticket_id TEXT,ordinal INTEGER,repository_key TEXT,ref_selector TEXT,access TEXT);CREATE TABLE typed_ticket_events(workspace_id TEXT,ticket_id TEXT,event_index INTEGER,kind TEXT,author TEXT,at TEXT,from_state TEXT,to_state TEXT,heading TEXT,body TEXT,PRIMARY KEY(workspace_id,ticket_id,event_index));CREATE TABLE typed_ticket_event_attributes(workspace_id TEXT,ticket_id TEXT,event_index INTEGER,key TEXT,value TEXT,PRIMARY KEY(workspace_id,ticket_id,event_index,key));INSERT INTO workspaces VALUES('W');INSERT INTO repositories VALUES('W','R');INSERT INTO repositories VALUES('W','R2');INSERT INTO ticket_current_worker_assignments VALUES('W','T','A','runtime','coder','t');INSERT INTO typed_tickets VALUES('W','T','inprogress',1,'t','Title','Requirements');CREATE VIEW ticket_active_worker_assignments AS SELECT current.* FROM ticket_current_worker_assignments current JOIN typed_tickets ticket ON ticket.workspace_id=current.workspace_id AND ticket.ticket_id=current.ticket_id WHERE ticket.workflow_state NOT IN ('done','closed');").unwrap();
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
/// Frozen v12 fixture: counters must never be promoted to content attestations.
fn freeze_v12_review_payloads(connection: &Connection) {
    let rows = {
        let mut statement = connection.prepare(
            "SELECT event_id,payload_json FROM merge_request_thread_events WHERE kind IN ('review_requested','review')",
        ).unwrap();
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    for (event_id, payload) in rows {
        let mut value: serde_json::Value = serde_json::from_str(&payload).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("ticket_content_digest");
        object.insert("ticket_item_revision".into(), serde_json::json!("T:0"));
        connection
            .execute(
                "UPDATE merge_request_thread_events SET payload_json=?2 WHERE event_id=?1",
                rusqlite::params![event_id, value.to_string()],
            )
            .unwrap();
    }
    connection
        .execute("UPDATE merge_request_schema SET version=12", [])
        .unwrap();
}

#[test]
fn v12_migration_revokes_old_grants_preserves_integration_and_requires_fresh_content_review() {
    let (dir, store) = fixture();
    open(&store);
    let approved = approve(&store, "source", "old-consumed");
    let completion = CompleteMergeRequest {
        merge_request_id: "MR".into(),
        ticket_id: "T".into(),
        operation_id: "integration".into(),
        approval_event_id: approved.event_id.clone(),
        current_subject_ref: "source".into(),
        target_ref_before: "before".into(),
        target_ref_after: "after".into(),
        strategy: MergeStrategy::FastForward,
        resolution: ConflictResolution::None,
        auth: auth(),
        now: at(5),
    };
    let merged = store.complete(completion.clone()).unwrap();
    request(&store, "source", "old-issued");
    let connection = Connection::open(dir.path().join("db")).unwrap();
    freeze_v12_review_payloads(&connection);
    let original_reviews = {
        let mut statement = connection.prepare(
            "SELECT event_id,payload_json FROM merge_request_thread_events WHERE kind IN ('review_requested','review') ORDER BY event_id",
        ).unwrap();
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    migrate(&connection).unwrap();
    let archived_reviews = {
        let mut statement = connection.prepare(
            "SELECT event_id,payload_json FROM merge_request_legacy_review_archive ORDER BY event_id",
        ).unwrap();
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(
        archived_reviews, original_reviews,
        "historical review bytes must survive conversion"
    );
    assert!(
        connection
            .execute_batch("UPDATE merge_request_legacy_review_archive SET payload_json='{}'")
            .is_err()
    );
    assert!(
        connection
            .execute_batch("DELETE FROM merge_request_legacy_review_archive")
            .is_err()
    );
    let grants: Vec<(String, String, Option<String>)> = connection.prepare(
        "SELECT capability_token,status,revoked_at FROM merge_request_review_grants ORDER BY capability_token",
    ).unwrap().query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap().collect::<Result<_, _>>().unwrap();
    assert_eq!(grants[0].0, "old-consumed");
    assert_eq!(grants[0].1, "consumed");
    assert_eq!(grants[1].0, "old-issued");
    assert_eq!(grants[1].1, "revoked");
    assert!(grants[1].2.is_some());
    let legacy_terms: i64 = connection.query_row(
        "SELECT COUNT(*) FROM merge_request_thread_events WHERE payload_json LIKE '%ticket_item_revision%'",
        [], |row| row.get(0),
    ).unwrap();
    assert_eq!(legacy_terms, 0);
    let saved = store.get_by_id("W", "MR").unwrap();
    assert_eq!(saved.merged_result().unwrap(), &merged);
    let integration = saved.integration_approval().unwrap();
    assert_eq!(integration.event_id, approved.event_id);
    assert!(integration.ticket_content_digest.is_empty());
    let snapshot = vec![MergeRequestReviewSubject {
        merge_request_id: "MR".into(),
        subject_ref: "source".into(),
    }];
    assert_eq!(
        requirement_approval(
            &[saved],
            &content_digest("Title"),
            &snapshot,
            Some(&approved.event_id)
        ),
        Err(MergeRequestEvidenceError::TicketContentMismatch),
    );
    assert!(matches!(
        store.authorize_review_submission("MR", "old-issued"),
        Err(MergeRequestError::Unauthorized(_)),
    ));
    assert!(
        store
            .submit_review(SubmitMergeRequestReview {
                merge_request_id: "MR".into(),
                ticket_id: "T".into(),
                current_subject_ref: "source".into(),
                capability_token: "old-issued".into(),
                decision: ReviewDecision::Approve,
                body: "late old review".into(),
                findings: vec![],
                now: at(6),
            })
            .is_err()
    );
    assert_eq!(
        store.complete(completion).unwrap(),
        merged,
        "recorded merge replay stays intact"
    );
    let fresh = approve(&store, "source", "fresh");
    assert_eq!(fresh.ticket_content_digest, content_digest("Title"));
    migrate(&connection).unwrap();
    let requests = store.list_for_ticket("W", "T").unwrap();
    assert_eq!(
        requirement_approval(
            &requests,
            &content_digest("Title"),
            &snapshot,
            Some(&fresh.event_id)
        )
        .unwrap(),
        &fresh,
    );
    assert_eq!(requests[0].merged_result().unwrap(), &merged);
    assert_eq!(
        connection
            .query_row("SELECT version FROM merge_request_schema", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        13
    );
}

#[test]
fn v12_migration_rolls_back_payload_and_grant_changes_when_schema_update_fails() {
    let (dir, store) = fixture();
    open(&store);
    request(&store, "source", "old-issued");
    let connection = Connection::open(dir.path().join("db")).unwrap();
    freeze_v12_review_payloads(&connection);
    let before: String = connection
        .query_row(
            "SELECT payload_json FROM merge_request_thread_events WHERE kind='review_requested'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    connection.execute_batch(
        "CREATE TRIGGER reject_schema_update BEFORE UPDATE ON merge_request_schema BEGIN SELECT RAISE(ABORT,'injected migration failure'); END;",
    ).unwrap();
    assert!(migrate(&connection).is_err());
    assert!(
        connection
            .prepare("SELECT * FROM merge_request_legacy_review_archive")
            .is_err(),
        "failed conversion must roll back its archive too"
    );
    assert_eq!(
        connection
            .query_row("SELECT version FROM merge_request_schema", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        12
    );
    assert_eq!(connection.query_row("SELECT payload_json FROM merge_request_thread_events WHERE kind='review_requested'", [], |row| row.get::<_, String>(0)).unwrap(), before);
    assert_eq!(connection.query_row("SELECT status FROM merge_request_review_grants WHERE capability_token='old-issued'", [], |row| row.get::<_, String>(0)).unwrap(), "issued");
    connection
        .execute_batch("DROP TRIGGER reject_schema_update")
        .unwrap();
    migrate(&connection).unwrap();
    assert!(
        store
            .authorize_review_submission("MR", "old-issued")
            .is_err()
    );
}

fn open_for(s: &MergeRequestStore, merge_request_id: &str, repository_id: &str) {
    s.open_merge_request(OpenMergeRequest {
        merge_request_id: merge_request_id.into(),
        ticket_id: "T".into(),
        repository_id: repository_id.into(),
        selector_from: if merge_request_id == "MR" {
            "work/t".into()
        } else {
            format!("work/{merge_request_id}")
        },
        selector_to: "develop".into(),
        summary: "summary".into(),
        auth: auth_repo(repository_id),
        now: at(1),
    })
    .unwrap();
}
fn open(s: &MergeRequestStore) {
    open_for(s, "MR", "R");
}
fn request_for(
    s: &MergeRequestStore,
    merge_request_id: &str,
    repository_id: &str,
    subject: &str,
    token: &str,
) -> ReviewRequestedEvent {
    s.register_reviewer_child_session(RegisterReviewerChildSession {
        workspace_id: "W".into(),
        parent_runtime_id: "runtime".into(),
        parent_worker_id: "coder".into(),
        child_session_id: format!("child-{token}"),
        reviewer_profile: "builtin:reviewer".into(),
        now: at(2),
    })
    .unwrap();
    let ticket_merge_request_subjects = s
        .list_for_ticket("W", "T")
        .unwrap()
        .into_iter()
        .map(|request| {
            let other_subject = match request.merge_request_id.as_str() {
                "MR" => "subject-one",
                "MR-2" => "subject-two",
                _ => subject,
            };
            MergeRequestReviewSubject {
                subject_ref: if request.merge_request_id == merge_request_id {
                    subject.into()
                } else {
                    other_subject.into()
                },
                merge_request_id: request.merge_request_id,
            }
        })
        .collect();
    s.request_review(RequestMergeRequestReview {
        merge_request_id: merge_request_id.into(),
        ticket_content_digest: content_digest("Title"),
        ticket_merge_request_subjects,
        ticket_id: "T".into(),
        subject_ref: subject.into(),
        child_session_id: format!("child-{token}"),
        capability_token: token.into(),
        auth: auth_repo(repository_id),
        now: at(3),
    })
    .unwrap()
    .request_event
}
fn request(s: &MergeRequestStore, subject: &str, token: &str) -> ReviewRequestedEvent {
    request_for(s, "MR", "R", subject, token)
}
fn approve_for(
    s: &MergeRequestStore,
    merge_request_id: &str,
    repository_id: &str,
    subject: &str,
    token: &str,
) -> ReviewEvent {
    request_for(s, merge_request_id, repository_id, subject, token);
    s.submit_review(SubmitMergeRequestReview {
        merge_request_id: merge_request_id.into(),
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
fn approve(s: &MergeRequestStore, subject: &str, token: &str) -> ReviewEvent {
    approve_for(s, "MR", "R", subject, token)
}
#[test]
fn review_submission_authorization_rejects_invalid_grants_before_side_effects() {
    let (_d, store) = fixture();
    open(&store);
    request(&store, "published-source", "valid-token");

    let invalid = store
        .authorize_review_submission("MR", "invalid-token")
        .unwrap_err();
    assert!(matches!(invalid, MergeRequestError::Unauthorized(_)));
    let authorized = store
        .authorize_review_submission("MR", "valid-token")
        .unwrap();
    assert_eq!(authorized.workspace_id, "W");
    assert_eq!(authorized.merge_request_id, "MR");
    assert_eq!(authorized.ticket_id, "T");
    assert_eq!(authorized.subject_ref, "published-source");
}

#[test]
fn integration_preserves_selectors_ticket_state_and_assignment_and_replays_its_merge() {
    let (d, s) = fixture();
    open(&s);
    let review = approve(&s, "opaque-source-ref", "token");
    let ready = s
        .readiness(ReadinessCheck {
            merge_request_id: "MR".into(),
            ticket_id: "T".into(),
            current_subject_ref: Some("opaque-source-ref".into()),
            auth: auth(),
        })
        .unwrap();
    assert!(ready.ready);
    let merged = s
        .complete(CompleteMergeRequest {
            merge_request_id: "MR".into(),
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
    assert!(current_assignment);
    let replayed = s
        .complete(CompleteMergeRequest {
            merge_request_id: "MR".into(),
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
    let connection = Connection::open(d.path().join("db")).unwrap();
    let (state, active): (String, bool) = connection.query_row(
        "SELECT workflow_state, EXISTS(SELECT 1 FROM ticket_active_worker_assignments WHERE workspace_id='W' AND ticket_id='T') FROM typed_tickets WHERE workspace_id='W' AND ticket_id='T'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert_eq!(state, "inprogress");
    assert!(active);
    let json = serde_json::to_string(&mr).unwrap();
    for banned in [
        "digest_id",
        "attempt_id",
        "base_commit",
        "head_commit",
        "source_commit",
        "result_commit",
        "current_digest",
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
            merge_request_id: "MR".into(),
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
            merge_request_id: "MR".into(),
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
            merge_request_id: "MR".into(),
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
            merge_request_id: "MR".into(),
            operation_id: "target-moved".into(),
            ticket_id: "T".into(),
            current_subject_ref: "source-2".into(),
            target_ref_before: "target-2".into(),
            target_ref_after: "integrated-target-2".into(),
            approval_event_id: second.event_id.clone(),
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
        merge_request_id: "MR".into(),
        ticket_id: "T".into(),
        review_event_id: review.event_id,
        reason: "bad evidence".into(),
        auth: auth(),
        now: at(7),
    })
    .unwrap();
    let r = s
        .readiness(ReadinessCheck {
            merge_request_id: "MR".into(),
            ticket_id: "T".into(),
            current_subject_ref: Some("source".into()),
            auth: auth(),
        })
        .unwrap();
    assert!(!r.ready);
}

#[test]
fn fresh_schema_uses_version_13_and_reopens_as_current() {
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
        13
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
            merge_request_id: "MR".into(),
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
        merge_request_id: "MR".into(),
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
            merge_request_id: "MR".into(),
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
        merge_request_id: "MR".into(),
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
            merge_request_id: "MR".into(),
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
        merge_request_id: "MR".into(),
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
        merge_request_id: "MR".into(),
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

#[test]
fn ticket_can_link_parallel_open_merge_requests_in_distinct_repositories() {
    let (_dir, store) = fixture();
    open_for(&store, "MR", "R");
    open_for(&store, "MR-2", "R2");

    let linked = store.list_for_ticket("W", "T").unwrap();
    assert_eq!(
        linked
            .iter()
            .map(|request| request.merge_request_id.as_str())
            .collect::<Vec<_>>(),
        vec!["MR", "MR-2"]
    );
    let duplicate = store.open_merge_request(OpenMergeRequest {
        merge_request_id: "MR-duplicate".into(),
        ticket_id: "T".into(),
        repository_id: "R2".into(),
        selector_from: "work/duplicate".into(),
        selector_to: "develop".into(),
        summary: String::new(),
        auth: auth_repo("R2"),
        now: at(2),
    });
    assert!(matches!(duplicate, Err(MergeRequestError::Conflict(_))));
}

#[test]
fn mr_operations_and_review_capabilities_are_bound_to_explicit_identity() {
    let (_dir, store) = fixture();
    open_for(&store, "MR", "R");
    open_for(&store, "MR-2", "R2");
    request_for(&store, "MR", "R", "subject-one", "token-one");

    assert!(matches!(
        store.authorize_review_submission("MR-2", "token-one"),
        Err(MergeRequestError::Unauthorized(_))
    ));
    assert!(matches!(
        store.submit_review(SubmitMergeRequestReview {
            merge_request_id: "MR-2".into(),
            ticket_id: "T".into(),
            current_subject_ref: "subject-one".into(),
            capability_token: "token-one".into(),
            decision: ReviewDecision::Approve,
            body: "wrong MR".into(),
            findings: vec![],
            now: at(4),
        }),
        Err(MergeRequestError::Unauthorized(_))
    ));
    assert!(
        !store
            .get_by_id("W", "MR-2")
            .unwrap()
            .thread
            .iter()
            .any(|event| matches!(event, MergeRequestThreadEvent::Review(_)))
    );
}

#[test]
fn ticket_rescope_requires_fresh_review_before_merge_and_remains_recoverable() {
    let (dir, store) = fixture();
    open(&store);
    let stale = approve(&store, "subject", "token-stale");
    let connection = Connection::open(dir.path().join("db")).unwrap();
    connection
        .execute_batch(
            "UPDATE typed_tickets SET title='Rescoped' WHERE workspace_id='W' AND ticket_id='T'; INSERT INTO typed_ticket_events VALUES('W','T',1,'item_edit','user','t2',NULL,NULL,NULL,NULL)",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO typed_ticket_event_attributes VALUES('W','T',1,'event_id','t2')",
            [],
        )
        .unwrap();
    drop(connection);

    let readiness = store
        .readiness(ReadinessCheck {
            merge_request_id: "MR".into(),
            ticket_id: "T".into(),
            current_subject_ref: Some("subject".into()),
            auth: auth(),
        })
        .unwrap();
    assert!(!readiness.ready);
    assert!(
        readiness
            .blockers
            .iter()
            .any(|blocker| blocker.contains("Ticket digest"))
    );

    let stale_completion = CompleteMergeRequest {
        merge_request_id: "MR".into(),
        ticket_id: "T".into(),
        operation_id: "merge-stale".into(),
        approval_event_id: stale.event_id,
        current_subject_ref: "subject".into(),
        target_ref_before: "target-before".into(),
        target_ref_after: "target-after".into(),
        strategy: MergeStrategy::FastForward,
        resolution: ConflictResolution::None,
        auth: auth(),
        now: at(5),
    };
    assert!(matches!(
        store.validate_completion(&stale_completion),
        Err(MergeRequestError::NotReady(_))
    ));
    assert!(matches!(
        store.complete(stale_completion),
        Err(MergeRequestError::NotReady(_))
    ));

    store
        .register_reviewer_child_session(RegisterReviewerChildSession {
            workspace_id: "W".into(),
            parent_runtime_id: "runtime".into(),
            parent_worker_id: "coder".into(),
            child_session_id: "child-fresh".into(),
            reviewer_profile: "builtin:reviewer".into(),
            now: at(6),
        })
        .unwrap();
    store
        .request_review(RequestMergeRequestReview {
            merge_request_id: "MR".into(),
            ticket_id: "T".into(),
            ticket_content_digest: content_digest("Rescoped"),
            ticket_merge_request_subjects: vec![MergeRequestReviewSubject {
                merge_request_id: "MR".into(),
                subject_ref: "subject".into(),
            }],
            subject_ref: "subject".into(),
            child_session_id: "child-fresh".into(),
            capability_token: "token-fresh".into(),
            auth: auth(),
            now: at(7),
        })
        .unwrap();
    let fresh = store
        .submit_review(SubmitMergeRequestReview {
            merge_request_id: "MR".into(),
            ticket_id: "T".into(),
            current_subject_ref: "subject".into(),
            capability_token: "token-fresh".into(),
            decision: ReviewDecision::Approve,
            body: "fresh requirements approved".into(),
            findings: vec![],
            now: at(8),
        })
        .unwrap();
    store
        .complete(CompleteMergeRequest {
            merge_request_id: "MR".into(),
            ticket_id: "T".into(),
            operation_id: "merge-fresh".into(),
            approval_event_id: fresh.event_id,
            current_subject_ref: "subject".into(),
            target_ref_before: "target-before".into(),
            target_ref_after: "target-after".into(),
            strategy: MergeStrategy::FastForward,
            resolution: ConflictResolution::None,
            auth: auth(),
            now: at(9),
        })
        .unwrap();
}

#[test]
fn ticket_rescope_after_integration_accepts_fresh_requirement_attestation() {
    let (dir, store) = fixture();
    open(&store);
    let integration_approval = approve(&store, "subject", "token-integration");
    let merge = store
        .complete(CompleteMergeRequest {
            merge_request_id: "MR".into(),
            ticket_id: "T".into(),
            operation_id: "merge".into(),
            approval_event_id: integration_approval.event_id.clone(),
            current_subject_ref: "subject".into(),
            target_ref_before: "target-before".into(),
            target_ref_after: "target-after".into(),
            strategy: MergeStrategy::FastForward,
            resolution: ConflictResolution::None,
            auth: auth(),
            now: at(5),
        })
        .unwrap();
    let connection = Connection::open(dir.path().join("db")).unwrap();
    connection
        .execute_batch(
            "UPDATE typed_tickets SET title='Rescoped' WHERE workspace_id='W' AND ticket_id='T'; INSERT INTO typed_ticket_events VALUES('W','T',1,'item_edit','user','t2',NULL,NULL,NULL,NULL)",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO typed_ticket_event_attributes VALUES('W','T',1,'event_id','t2')",
            [],
        )
        .unwrap();
    drop(connection);

    let snapshot = vec![MergeRequestReviewSubject {
        merge_request_id: "MR".into(),
        subject_ref: merge.approved_source_ref.clone(),
    }];
    let before_refresh = store.list_for_ticket("W", "T").unwrap();
    assert_eq!(
        requirement_approval(
            &before_refresh,
            &content_digest("Rescoped"),
            &snapshot,
            Some(&integration_approval.event_id),
        ),
        Err(MergeRequestEvidenceError::TicketContentMismatch)
    );
    assert_eq!(before_refresh[0].merged_result().unwrap(), &merge);
    assert_eq!(
        before_refresh[0].integration_approval().unwrap(),
        &integration_approval
    );

    store
        .register_reviewer_child_session(RegisterReviewerChildSession {
            workspace_id: "W".into(),
            parent_runtime_id: "runtime".into(),
            parent_worker_id: "coder".into(),
            child_session_id: "child-post-merge".into(),
            reviewer_profile: "builtin:reviewer".into(),
            now: at(6),
        })
        .unwrap();
    store
        .request_review(RequestMergeRequestReview {
            merge_request_id: "MR".into(),
            ticket_id: "T".into(),
            ticket_content_digest: content_digest("Rescoped"),
            ticket_merge_request_subjects: vec![MergeRequestReviewSubject {
                merge_request_id: "MR".into(),
                subject_ref: "subject".into(),
            }],
            subject_ref: "subject".into(),
            child_session_id: "child-post-merge".into(),
            capability_token: "token-post-merge".into(),
            auth: auth(),
            now: at(7),
        })
        .unwrap();
    let requirement_review = store
        .submit_review(SubmitMergeRequestReview {
            merge_request_id: "MR".into(),
            ticket_id: "T".into(),
            current_subject_ref: "subject".into(),
            capability_token: "token-post-merge".into(),
            decision: ReviewDecision::Approve,
            body: "rescope requirements approved against merged result".into(),
            findings: vec![],
            now: at(8),
        })
        .unwrap();
    let refreshed = store.list_for_ticket("W", "T").unwrap();
    assert_eq!(
        requirement_approval(
            &refreshed,
            &content_digest("Rescoped"),
            &snapshot,
            Some(&requirement_review.event_id),
        )
        .unwrap(),
        &requirement_review
    );
    assert_eq!(
        requirement_approval(&refreshed, &content_digest("Rescoped"), &snapshot, None).unwrap(),
        &requirement_review
    );
    assert_eq!(refreshed[0].merged_result().unwrap(), &merge);
    assert_eq!(
        refreshed[0].integration_approval().unwrap(),
        &integration_approval
    );
    let connection = Connection::open(dir.path().join("db")).unwrap();
    let merge_payload: String = connection.query_row(
        "SELECT payload_json FROM merge_request_thread_events WHERE workspace_id='W' AND merge_request_id='MR' AND kind='merge'",
        [],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(merge_payload, serde_json::to_string(&merge).unwrap());
}

#[test]
fn review_request_rejects_a_snapshot_that_omits_a_linked_merge_request() {
    let (_dir, store) = fixture();
    open_for(&store, "MR", "R");
    open_for(&store, "MR-2", "R2");
    store
        .register_reviewer_child_session(RegisterReviewerChildSession {
            workspace_id: "W".into(),
            parent_runtime_id: "runtime".into(),
            parent_worker_id: "coder".into(),
            child_session_id: "child-incomplete".into(),
            reviewer_profile: "builtin:reviewer".into(),
            now: at(3),
        })
        .unwrap();
    let result = store.request_review(RequestMergeRequestReview {
        merge_request_id: "MR".into(),
        ticket_id: "T".into(),
        ticket_content_digest: content_digest("Title"),
        ticket_merge_request_subjects: vec![MergeRequestReviewSubject {
            merge_request_id: "MR".into(),
            subject_ref: "subject-one".into(),
        }],
        subject_ref: "subject-one".into(),
        child_session_id: "child-incomplete".into(),
        capability_token: "token-incomplete".into(),
        auth: auth(),
        now: at(4),
    });
    assert!(matches!(result, Err(MergeRequestError::Conflict(_))));
}

#[test]
fn partial_and_full_integration_retain_ticket_state_and_active_assignment() {
    let (dir, store) = fixture();
    open_for(&store, "MR", "R");
    open_for(&store, "MR-2", "R2");
    let first = approve_for(&store, "MR", "R", "subject-one", "token-one");
    let second = approve_for(&store, "MR-2", "R2", "subject-two", "token-two");

    let first_merge = store
        .complete(CompleteMergeRequest {
            merge_request_id: "MR".into(),
            ticket_id: "T".into(),
            operation_id: "merge-one".into(),
            approval_event_id: first.event_id.clone(),
            current_subject_ref: "subject-one".into(),
            target_ref_before: "target-one-before".into(),
            target_ref_after: "target-one-after".into(),
            strategy: MergeStrategy::FastForward,
            resolution: ConflictResolution::None,
            auth: auth_repo("R"),
            now: at(5),
        })
        .unwrap();
    let partial = store.get_by_id("W", "MR-2").unwrap();
    assert_eq!(
        partial.integration_approval(),
        Err(MergeRequestEvidenceError::NotMerged)
    );

    let connection = Connection::open(dir.path().join("db")).unwrap();
    let assert_ticket_unchanged = || {
        let (state, assigned, active, state_changes): (String, bool, bool, i64) = connection
            .query_row(
                "SELECT workflow_state,
                    EXISTS(SELECT 1 FROM ticket_current_worker_assignments WHERE workspace_id='W' AND ticket_id='T'),
                    EXISTS(SELECT 1 FROM ticket_active_worker_assignments WHERE workspace_id='W' AND ticket_id='T'),
                    (SELECT COUNT(*) FROM typed_ticket_events WHERE workspace_id='W' AND ticket_id='T' AND kind='state_changed')
                 FROM typed_tickets WHERE workspace_id='W' AND ticket_id='T'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(state, "inprogress");
        assert!(assigned);
        assert!(active);
        assert_eq!(state_changes, 0);
    };
    assert_ticket_unchanged();

    let second_merge = store
        .complete(CompleteMergeRequest {
            merge_request_id: "MR-2".into(),
            ticket_id: "T".into(),
            operation_id: "merge-two".into(),
            approval_event_id: second.event_id.clone(),
            current_subject_ref: "subject-two".into(),
            target_ref_before: "target-two-before".into(),
            target_ref_after: "target-two-after".into(),
            strategy: MergeStrategy::FastForward,
            resolution: ConflictResolution::None,
            auth: auth_repo("R2"),
            now: at(7),
        })
        .unwrap();
    assert_ticket_unchanged();
    let requests = store.list_for_ticket("W", "T").unwrap();
    let snapshot = requests
        .iter()
        .map(|request| MergeRequestReviewSubject {
            merge_request_id: request.merge_request_id.clone(),
            subject_ref: request.merged_result().unwrap().approved_source_ref.clone(),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        requirement_approval(
            &requests,
            &content_digest("Title"),
            &snapshot,
            Some(&second.event_id)
        )
        .unwrap(),
        &second
    );
    let first_request = requests
        .iter()
        .find(|request| request.merge_request_id == "MR")
        .unwrap();
    let second_request = requests
        .iter()
        .find(|request| request.merge_request_id == "MR-2")
        .unwrap();
    assert_eq!(first_request.merged_result().unwrap(), &first_merge);
    assert_eq!(second_request.merged_result().unwrap(), &second_merge);
    assert_eq!(first_request.integration_approval().unwrap(), &first);
    assert_eq!(second_request.integration_approval().unwrap(), &second);
}

#[test]
fn historical_ticket_completion_payload_remains_readable_and_unchanged_on_migration() {
    let (dir, _store) = fixture();
    let connection = Connection::open(dir.path().join("db")).unwrap();
    let payload = r#"{
        "operation_id": "historical-operation",
        "ticket_id": "T",
        "item_revision": "historical-revision",
        "merge_request_ids": ["MR-2", "MR"],
        "requirement_approval_event_id": "historical-review",
        "completed_by": {"runtime_id": "runtime", "worker_id": "coder"},
        "created_at": "2026-07-26T12:00:07Z"
    }"#;
    connection.execute_batch(
        "UPDATE typed_tickets SET workflow_state='done' WHERE workspace_id='W' AND ticket_id='T';
         INSERT INTO typed_ticket_events VALUES('W','T',0,'state_changed','worker:runtime:coder','2026-07-26T12:00:07Z','inprogress','done','Ticket requirements completed','historical body');"
    ).unwrap();
    connection
        .execute(
            "INSERT INTO typed_ticket_event_attributes VALUES('W','T',0,'ticket_completion',?1)",
            [payload],
        )
        .unwrap();

    migrate(&connection).unwrap();

    let (saved_payload, kind, from_state, to_state, heading, body, state): (String, String, String, String, String, String, String) = connection.query_row(
        "SELECT attribute.value,event.kind,event.from_state,event.to_state,event.heading,event.body,ticket.workflow_state
           FROM typed_ticket_event_attributes attribute
           JOIN typed_ticket_events event USING(workspace_id,ticket_id,event_index)
           JOIN typed_tickets ticket USING(workspace_id,ticket_id)
          WHERE attribute.workspace_id='W' AND attribute.ticket_id='T' AND attribute.key='ticket_completion'",
        [],
        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?)),
    ).unwrap();
    assert_eq!(saved_payload, payload);
    assert_eq!(kind, "state_changed");
    assert_eq!(from_state, "inprogress");
    assert_eq!(to_state, "done");
    assert_eq!(heading, "Ticket requirements completed");
    assert_eq!(body, "historical body");
    assert_eq!(state, "done");
    let historical: TicketCompletionEvent = serde_json::from_str(&saved_payload).unwrap();
    assert_eq!(
        serde_json::to_value(&historical).unwrap(),
        serde_json::from_str::<serde_json::Value>(payload).unwrap()
    );
}

#[test]
fn persisted_integration_evidence_rejects_missing_mismatched_revoked_or_late_approval() {
    for (case, expected) in [
        (
            "merge-missing",
            MergeRequestEvidenceError::MergeResultMissing,
        ),
        (
            "approval-missing",
            MergeRequestEvidenceError::ApprovalMissing,
        ),
        ("revoked", MergeRequestEvidenceError::ApprovalRevoked),
        (
            "source-mismatch",
            MergeRequestEvidenceError::ApprovalSourceMismatch,
        ),
        (
            "not-approved",
            MergeRequestEvidenceError::ApprovalNotApproved,
        ),
        (
            "request-missing",
            MergeRequestEvidenceError::ReviewRequestMissing,
        ),
        (
            "request-mismatch",
            MergeRequestEvidenceError::ReviewRequestMismatch,
        ),
        (
            "approval-after-merge",
            MergeRequestEvidenceError::ApprovalTimelineMismatch,
        ),
    ] {
        let (dir, store) = fixture();
        open(&store);
        let review = approve(&store, "merged-source", "integration-token");
        let merge = store
            .complete(CompleteMergeRequest {
                merge_request_id: "MR".into(),
                ticket_id: "T".into(),
                operation_id: "merge".into(),
                approval_event_id: review.event_id.clone(),
                current_subject_ref: "merged-source".into(),
                target_ref_before: "before".into(),
                target_ref_after: "after".into(),
                strategy: MergeStrategy::FastForward,
                resolution: ConflictResolution::None,
                auth: auth(),
                now: at(5),
            })
            .unwrap();
        let persisted = store.get_by_id("W", "MR").unwrap();
        assert_eq!(persisted.merged_result().unwrap(), &merge);
        assert_eq!(persisted.integration_approval().unwrap(), &review);

        let connection = Connection::open(dir.path().join("db")).unwrap();
        match case {
            "merge-missing" | "approval-missing" | "request-missing" => {
                let kind = match case {
                    "merge-missing" => "merge",
                    "approval-missing" => "review",
                    _ => "review_requested",
                };
                connection.execute(
                    "DELETE FROM merge_request_thread_events WHERE workspace_id='W' AND merge_request_id='MR' AND kind=?1",
                    [kind],
                ).unwrap();
            }
            "revoked" => {
                store
                    .revoke_review(RevokeMergeRequestReview {
                        merge_request_id: "MR".into(),
                        ticket_id: "T".into(),
                        review_event_id: review.event_id.clone(),
                        reason: "invalid integration evidence".into(),
                        auth: auth(),
                        now: at(6),
                    })
                    .unwrap();
            }
            "approval-after-merge" => {
                let mut altered = review.clone();
                altered.sequence = merge.sequence + 1;
                connection.execute(
                    "UPDATE merge_request_thread_events SET payload_json=?1,sequence=?2 WHERE workspace_id='W' AND merge_request_id='MR' AND kind='review'",
                    rusqlite::params![serde_json::to_string(&altered).unwrap(), altered.sequence],
                ).unwrap();
            }
            _ => {
                let mut altered = review.clone();
                match case {
                    "source-mismatch" => altered.subject_ref = "different-source".into(),
                    "not-approved" => altered.decision = ReviewDecision::RequestChanges,
                    "request-mismatch" => altered.ticket_content_digest = "different-digest".into(),
                    _ => unreachable!(),
                }
                connection.execute(
                    "UPDATE merge_request_thread_events SET payload_json=?1 WHERE workspace_id='W' AND merge_request_id='MR' AND kind='review'",
                    [serde_json::to_string(&altered).unwrap()],
                ).unwrap();
            }
        }
        let reloaded = store.get_by_id("W", "MR").unwrap();
        assert_eq!(reloaded.integration_approval(), Err(expected), "{case}");
        let (state, assigned): (String, bool) = connection.query_row(
            "SELECT workflow_state, EXISTS(SELECT 1 FROM ticket_current_worker_assignments WHERE workspace_id='W' AND ticket_id='T') FROM typed_tickets WHERE workspace_id='W' AND ticket_id='T'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(state, "inprogress", "{case}");
        assert!(assigned, "{case}");
    }
}
