use merge_request::*;
use rusqlite::{Connection, params};
use std::sync::{Arc, Barrier};
use std::thread;
use tempfile::TempDir;

fn setup() -> (TempDir, SqliteMergeRequestStore) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(r#"
      PRAGMA foreign_keys=ON;
      CREATE TABLE repositories(workspace_id TEXT NOT NULL,repository_id TEXT NOT NULL,PRIMARY KEY(workspace_id,repository_id));
      CREATE TABLE typed_tickets(workspace_id TEXT NOT NULL,ticket_id TEXT NOT NULL,workflow_state TEXT NOT NULL,workflow_state_explicit INTEGER NOT NULL DEFAULT 1,updated_at TEXT NOT NULL,PRIMARY KEY(workspace_id,ticket_id));
      CREATE TABLE typed_ticket_events(workspace_id TEXT NOT NULL,ticket_id TEXT NOT NULL,event_index INTEGER NOT NULL,kind TEXT NOT NULL,author TEXT,at TEXT,status TEXT,from_state TEXT,to_state TEXT,heading TEXT,body TEXT,PRIMARY KEY(workspace_id,ticket_id,event_index));
      CREATE TABLE typed_ticket_event_attributes(workspace_id TEXT NOT NULL,ticket_id TEXT NOT NULL,event_index INTEGER NOT NULL,key TEXT NOT NULL,value TEXT NOT NULL,PRIMARY KEY(workspace_id,ticket_id,event_index,key));
      CREATE TABLE ticket_worker_assignments(workspace_id TEXT NOT NULL,ticket_id TEXT NOT NULL,assignment_id TEXT NOT NULL,runtime_id TEXT NOT NULL,worker_id TEXT NOT NULL,PRIMARY KEY(workspace_id,ticket_id,assignment_id));
      CREATE TABLE ticket_current_worker_assignments(workspace_id TEXT NOT NULL,ticket_id TEXT NOT NULL,assignment_id TEXT NOT NULL,runtime_id TEXT NOT NULL,worker_id TEXT NOT NULL,PRIMARY KEY(workspace_id,ticket_id));
    "#).unwrap();
    for ws in ["ws-a", "ws-b"] {
        conn.execute("INSERT INTO repositories VALUES(?1,'repo')", params![ws])
            .unwrap();
        conn.execute(
            "INSERT INTO typed_tickets VALUES(?1,'T1','inprogress',1,'t0')",
            params![ws],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ticket_worker_assignments VALUES(?1,'T1','A1','R1','W1')",
            params![ws],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ticket_current_worker_assignments VALUES(?1,'T1','A1','R1','W1')",
            params![ws],
        )
        .unwrap();
    }
    drop(conn);
    let store = SqliteMergeRequestStore::open(&path, "ws-a").unwrap();
    (dir, store)
}

fn revision(id: &str, ordinal: u64, head: &str) -> MergeRequestRevision {
    MergeRequestRevision {
        revision_id: id.into(),
        ordinal,
        base_commit: "base".into(),
        head_commit: head.into(),
        changed_paths: vec!["src/lib.rs".into()],
        summary: format!("revision {id}"),
        assignment_id: "A1".into(),
        created_at: format!("t{ordinal}"),
    }
}

fn open(store: &SqliteMergeRequestStore) {
    store
        .open_merge_request(OpenMergeRequest {
            merge_request_id: "MR1".into(),
            ticket_id: "T1".into(),
            repository_id: "repo".into(),
            target_ref_selector: "refs/heads/develop".into(),
            revision: revision("V1", 1, "head"),
            authenticated_runtime_id: "R1".into(),
            authenticated_worker_id: "W1".into(),
            now: "t1".into(),
        })
        .unwrap();
}

fn attempt(store: &SqliteMergeRequestStore, revision: &str, token: &str) {
    let child = format!("child-{revision}");
    store
        .register_reviewer_child_session(RegisterReviewerChildSession {
            parent_runtime_id: "R1".into(),
            parent_worker_id: "W1".into(),
            child_session_id: child.clone(),
            now: "t2".into(),
        })
        .unwrap();
    store
        .register_review_attempt(RegisterReviewAttempt {
            attempt_id: format!("attempt-{revision}"),
            ticket_id: "T1".into(),
            revision_id: revision.into(),
            parent_assignment_id: "A1".into(),
            parent_runtime_id: "R1".into(),
            parent_worker_id: "W1".into(),
            child_session_id: child,
            capability_token: token.into(),
            now: "t2".into(),
        })
        .unwrap();
}

fn approve(store: &SqliteMergeRequestStore, revision: &str, token: &str) {
    attempt(store, revision, token);
    store
        .submit_review(SubmitReview {
            ticket_id: "T1".into(),
            revision_id: revision.into(),
            capability_token: token.into(),
            decision: ReviewDecision::Approve,
            body: "approved".into(),
            findings: vec![],
            now: "t3".into(),
        })
        .unwrap();
}

fn completion(operation_id: &str) -> CompleteMergeRequest {
    CompleteMergeRequest {
        operation_id: operation_id.into(),
        ticket_id: "T1".into(),
        expected_revision_id: "V1".into(),
        target_commit: "base".into(),
        source_commit: "head".into(),
        result_commit: "head".into(),
        strategy: MergeStrategy::FastForward,
        resolution: MergeResolution::None,
        implementation_assignment_id: "A1".into(),
        completion_actor_runtime_id: "OR".into(),
        completion_actor_worker_id: "OW".into(),
        now: "t4".into(),
    }
}

#[test]
fn target_movement_does_not_invalidate_source_revision_approval() {
    let (_dir, store) = setup();
    open(&store);
    approve(&store, "V1", "token-v1");
    for target in ["base", "advanced-target"] {
        let readiness = store
            .readiness_for_ticket_with_target("T1", Some(target))
            .unwrap();
        assert!(
            readiness.ready,
            "target movement must not invalidate source approval"
        );
        assert_eq!(readiness.review_status, ReviewStatus::Approved);
        assert_eq!(readiness.observed_target_commit.as_deref(), Some(target));
    }
    store
        .add_revision(AddRevision {
            ticket_id: "T1".into(),
            expected_current_revision_id: "V1".into(),
            revision: revision("V2", 2, "head2"),
            authenticated_runtime_id: "R1".into(),
            authenticated_worker_id: "W1".into(),
            now: "t5".into(),
        })
        .unwrap();
    assert_eq!(
        store.readiness_for_ticket("T1").unwrap().review_status,
        ReviewStatus::Pending
    );
}

#[test]
fn completion_records_one_final_merge_outcome_and_replays_idempotently() {
    let (_dir, store) = setup();
    open(&store);
    approve(&store, "V1", "token-v1");
    let first = store.complete(completion("OP1")).unwrap();
    assert!(!first.replayed);
    assert_eq!(first.ticket_state, "done");
    let merged = store.show_for_ticket("T1").unwrap().unwrap();
    assert_eq!(merged.state, MergeRequestState::Merged);
    assert_eq!(merged.merged_revision_id.as_deref(), Some("V1"));
    assert_eq!(merged.merged_target_commit.as_deref(), Some("base"));
    assert_eq!(merged.merged_result_commit.as_deref(), Some("head"));
    assert_eq!(merged.merge_strategy, Some(MergeStrategy::FastForward));
    assert_eq!(merged.merge_resolution, Some(MergeResolution::None));
    assert_eq!(merged.merged_by_runtime_id.as_deref(), Some("OR"));
    assert_eq!(merged.merged_by_worker_id.as_deref(), Some("OW"));
    assert!(store.complete(completion("OP1")).unwrap().replayed);
    let mut conflicting = completion("OP1");
    conflicting.target_commit = "other".into();
    assert!(matches!(
        store.complete(conflicting),
        Err(MergeRequestError::OperationConflict)
    ));
}

#[test]
fn completion_rejects_invalid_or_non_current_source_outcomes_without_side_effects() {
    let (_dir, store) = setup();
    open(&store);
    approve(&store, "V1", "token-v1");
    let mut invalid_ff = completion("bad-ff");
    invalid_ff.result_commit = "different".into();
    assert!(matches!(
        store.complete(invalid_ff),
        Err(MergeRequestError::InvalidMergeOutcome(_))
    ));
    let mut invalid_merge = completion("bad-merge");
    invalid_merge.strategy = MergeStrategy::Merge;
    assert!(matches!(
        store.complete(invalid_merge),
        Err(MergeRequestError::InvalidMergeOutcome(_))
    ));
    let mut wrong_source = completion("wrong-source");
    wrong_source.source_commit = "not-approved".into();
    wrong_source.result_commit = "not-approved".into();
    assert!(matches!(
        store.complete(wrong_source),
        Err(MergeRequestError::InvalidMergeOutcome(_))
    ));
    assert_eq!(
        store.show_for_ticket("T1").unwrap().unwrap().state,
        MergeRequestState::Open
    );
    let conn = Connection::open(store.db_path()).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT workflow_state FROM typed_tickets WHERE workspace_id='ws-a' AND ticket_id='T1'",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        "inprogress"
    );
}

#[test]
fn concurrent_completion_converges_on_one_operation() {
    let (_dir, store) = setup();
    open(&store);
    approve(&store, "V1", "token-v1");
    let path = store.db_path().to_path_buf();
    let barrier = Arc::new(Barrier::new(3));
    let mut handles = Vec::new();
    for _ in 0..2 {
        let path = path.clone();
        let barrier = barrier.clone();
        handles.push(thread::spawn(move || {
            let store = SqliteMergeRequestStore::open_verified(path, "ws-a").unwrap();
            barrier.wait();
            store.complete(completion("OP-concurrent"))
        }));
    }
    barrier.wait();
    let outcomes: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap().unwrap())
        .collect();
    assert_eq!(
        outcomes.iter().filter(|outcome| !outcome.replayed).count(),
        1
    );
    assert_eq!(
        outcomes.iter().filter(|outcome| outcome.replayed).count(),
        1
    );
}

#[test]
fn reviewer_attempt_is_bound_to_direct_child_and_current_assignment() {
    let (_dir, store) = setup();
    open(&store);
    store
        .register_reviewer_child_session(RegisterReviewerChildSession {
            parent_runtime_id: "R1".into(),
            parent_worker_id: "W1".into(),
            child_session_id: "child".into(),
            now: "t2".into(),
        })
        .unwrap();
    store
        .register_review_attempt(RegisterReviewAttempt {
            attempt_id: "attempt".into(),
            ticket_id: "T1".into(),
            revision_id: "V1".into(),
            parent_assignment_id: "A1".into(),
            parent_runtime_id: "R1".into(),
            parent_worker_id: "W1".into(),
            child_session_id: "child".into(),
            capability_token: "token".into(),
            now: "t2".into(),
        })
        .unwrap();
    let wrong_token = store.submit_review(SubmitReview {
        ticket_id: "T1".into(),
        revision_id: "V1".into(),
        capability_token: "wrong".into(),
        decision: ReviewDecision::Approve,
        body: "approved".into(),
        findings: vec![],
        now: "t3".into(),
    });
    assert!(matches!(
        wrong_token,
        Err(MergeRequestError::InvalidReviewAttempt)
    ));
}
