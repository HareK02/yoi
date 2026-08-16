use std::sync::{Arc, Mutex};

use chrono::{TimeZone, Utc};
use merge_request::{
    AssignmentSource, CompleteMergeRequest, ConflictResolution, CurrentAssignment, FindingSeverity,
    MergeRequestAuth, MergeRequestState, MergeRequestStore, MergeRequestThreadEvent, MergeStrategy,
    OpenMergeRequest, ReadinessCheck, RegisterReviewCapability, RegisterReviewerChildSession,
    RepositorySource, RequestForReview, RequestMergeRequestReview, ReviewDecision, ReviewFinding,
    SubmitMergeRequestReview,
};
use rusqlite::{Connection, params};

#[derive(Clone)]
struct Assignments {
    current: Arc<Mutex<CurrentAssignment>>,
}

impl AssignmentSource for Assignments {
    fn current_assignment(
        &self,
        _workspace_id: &str,
        _ticket_id: &str,
    ) -> Result<Option<CurrentAssignment>, String> {
        Ok(Some(self.current.lock().unwrap().clone()))
    }
}

struct Repositories;

impl RepositorySource for Repositories {
    fn repository_belongs_to_workspace(
        &self,
        workspace_id: &str,
        repository_id: &str,
    ) -> Result<bool, String> {
        Ok(workspace_id == "W" && repository_id == "R")
    }

    fn is_ancestor(
        &self,
        _workspace_id: &str,
        _repository_id: &str,
        ancestor: &str,
        descendant: &str,
    ) -> Result<bool, String> {
        Ok(matches!(
            (ancestor, descendant),
            ("base", "head-1") | ("base", "head-2")
        ))
    }
}

fn now(second: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 26, 12, 0, second)
        .single()
        .unwrap()
}

fn fixture() -> (tempfile::TempDir, MergeRequestStore, Assignments) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         CREATE TABLE workspaces (workspace_id TEXT PRIMARY KEY);
         CREATE TABLE repositories (
            workspace_id TEXT NOT NULL,
            repository_id TEXT NOT NULL,
            PRIMARY KEY (workspace_id, repository_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id)
         );
         CREATE TABLE typed_tickets (
            workspace_id TEXT NOT NULL,
            ticket_id TEXT NOT NULL,
            workflow_state TEXT NOT NULL,
            workflow_state_explicit INTEGER NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, ticket_id)
         );
         CREATE TABLE typed_ticket_events (
            workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, event_index INTEGER NOT NULL,
            kind TEXT NOT NULL, author TEXT NOT NULL, at TEXT NOT NULL,
            from_state TEXT, to_state TEXT, heading TEXT, body TEXT,
            PRIMARY KEY (workspace_id, ticket_id, event_index)
         );
         CREATE TABLE typed_ticket_event_attributes (
            workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, event_index INTEGER NOT NULL,
            key TEXT NOT NULL, value TEXT NOT NULL,
            PRIMARY KEY (workspace_id, ticket_id, event_index, key)
         );
         INSERT INTO workspaces VALUES ('W');
         INSERT INTO repositories VALUES ('W', 'R');
         INSERT INTO typed_tickets VALUES ('W', 'T', 'inprogress', 1, '2026-07-26T12:00:00Z');",
    )
    .unwrap();
    drop(conn);
    let assignments = Assignments {
        current: Arc::new(Mutex::new(CurrentAssignment {
            assignment_id: "A1".into(),
            ticket_id: "T".into(),
            runtime_id: "runtime".into(),
            worker_id: "coder".into(),
        })),
    };
    let store =
        MergeRequestStore::open(&path, Arc::new(assignments.clone()), Arc::new(Repositories))
            .unwrap();
    (dir, store, assignments)
}

fn auth(assignment_id: &str) -> MergeRequestAuth {
    MergeRequestAuth {
        workspace_id: "W".into(),
        repository_id: "R".into(),
        runtime_id: "runtime".into(),
        worker_id: "coder".into(),
        assignment_id: assignment_id.into(),
    }
}

fn open(store: &MergeRequestStore) {
    store
        .open_merge_request(OpenMergeRequest {
            merge_request_id: "MR".into(),
            ticket_id: "T".into(),
            repository_id: "R".into(),
            selector_from: "work/t-feature".into(),
            selector_to: "develop".into(),
            request: RequestForReview {
                base_commit: "base".into(),
                head_commit: "head-1".into(),
                changed_paths: vec!["src/lib.rs".into()],
                summary: "first candidate".into(),
            },
            auth: auth("A1"),
            now: now(1),
        })
        .unwrap();
}

fn approve(store: &MergeRequestStore, expected_head_commit: &str, token: &str) {
    store
        .register_reviewer_child_session(RegisterReviewerChildSession {
            workspace_id: "W".into(),
            parent_runtime_id: "runtime".into(),
            parent_worker_id: "coder".into(),
            child_session_id: format!("child-{token}"),
            reviewer_profile: "builtin:reviewer".into(),
            now: now(2),
        })
        .unwrap();
    store
        .register_review_capability(RegisterReviewCapability {
            ticket_id: "T".into(),
            expected_head_commit: expected_head_commit.into(),
            child_session_id: format!("child-{token}"),
            capability_token: token.into(),
            auth: auth("A1"),
            now: now(3),
        })
        .unwrap();
    store
        .submit_review(SubmitMergeRequestReview {
            ticket_id: "T".into(),
            expected_head_commit: expected_head_commit.into(),
            capability_token: token.into(),
            decision: ReviewDecision::Approve,
            body: "approved independently".into(),
            findings: vec![ReviewFinding {
                severity: FindingSeverity::Note,
                path: None,
                line: None,
                message: "looks good".into(),
            }],
            now: now(4),
        })
        .unwrap();
}

#[test]
fn thread_drives_review_readiness_and_completion_without_public_revision_identity() {
    let (_dir, store, _assignments) = fixture();
    open(&store);
    approve(&store, "head-1", "token-1");

    let readiness = store
        .readiness(ReadinessCheck {
            ticket_id: "T".into(),
            expected_head_commit: Some("head-1".into()),
            auth: auth("A1"),
        })
        .unwrap();
    assert!(readiness.ready, "{:?}", readiness.blockers);

    let merged = store
        .complete(CompleteMergeRequest {
            ticket_id: "T".into(),
            expected_head_commit: "head-1".into(),
            operation_id: "op-1".into(),
            target_commit: "base".into(),
            source_commit: "head-1".into(),
            result_commit: "head-1".into(),
            strategy: MergeStrategy::FastForward,
            resolution: ConflictResolution::None,
            auth: auth("A1"),
            now: now(5),
        })
        .unwrap();
    assert_eq!(merged.result_commit, "head-1");

    let mr = store.get("W", "T").unwrap();
    assert_eq!(mr.state, MergeRequestState::Merged);
    assert!(matches!(
        mr.thread.last(),
        Some(MergeRequestThreadEvent::Merge(_))
    ));
    assert_eq!(mr.selector_from, "work/t-feature");
    assert_eq!(mr.selector_to, "develop");
    let json = serde_json::to_string(&mr).unwrap();
    for forbidden in [
        "revision_id",
        "current_revision",
        "attempt_id",
        "review_attempt",
        "head_tree",
        "diff_digest",
        "merged_revision_id",
    ] {
        assert!(
            !json.contains(forbidden),
            "unexpected `{forbidden}` in {json}"
        );
    }
}

#[test]
fn new_review_request_invalidates_prior_approval_and_fences_stale_capability() {
    let (_dir, store, _assignments) = fixture();
    open(&store);
    approve(&store, "head-1", "token-1");
    store
        .request_review(RequestMergeRequestReview {
            ticket_id: "T".into(),
            expected_head_commit: "head-1".into(),
            request: RequestForReview {
                base_commit: "base".into(),
                head_commit: "head-2".into(),
                changed_paths: vec!["src/lib.rs".into(), "tests/store.rs".into()],
                summary: "address review".into(),
            },
            auth: auth("A1"),
            now: now(6),
        })
        .unwrap();

    let readiness = store
        .readiness(ReadinessCheck {
            ticket_id: "T".into(),
            expected_head_commit: Some("head-2".into()),
            auth: auth("A1"),
        })
        .unwrap();
    assert!(!readiness.ready);
    assert!(
        readiness
            .blockers
            .iter()
            .any(|value| value.contains("no review result"))
    );
    assert!(
        store
            .submit_review(SubmitMergeRequestReview {
                ticket_id: "T".into(),
                expected_head_commit: "head-1".into(),
                capability_token: "token-1".into(),
                decision: ReviewDecision::Approve,
                body: "stale".into(),
                findings: vec![],
                now: now(7),
            })
            .is_err()
    );
}

#[test]
fn assignment_change_rejects_candidate_mutation() {
    let (_dir, store, assignments) = fixture();
    open(&store);
    *assignments.current.lock().unwrap() = CurrentAssignment {
        assignment_id: "A2".into(),
        ticket_id: "T".into(),
        runtime_id: "runtime".into(),
        worker_id: "other".into(),
    };
    let error = store
        .request_review(RequestMergeRequestReview {
            ticket_id: "T".into(),
            expected_head_commit: "head-1".into(),
            request: RequestForReview {
                base_commit: "base".into(),
                head_commit: "head-2".into(),
                changed_paths: vec![],
                summary: String::new(),
            },
            auth: auth("A1"),
            now: now(6),
        })
        .unwrap_err();
    assert!(error.to_string().contains("current assigned worker"));
}

#[test]
fn v11_migration_builds_thread_events_and_removes_revision_tables() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         CREATE TABLE workspaces (workspace_id TEXT PRIMARY KEY);
         CREATE TABLE repositories (
            workspace_id TEXT NOT NULL, repository_id TEXT NOT NULL,
            PRIMARY KEY (workspace_id, repository_id)
         );
         INSERT INTO workspaces VALUES ('W');
         INSERT INTO repositories VALUES ('W', 'R');
         CREATE TABLE typed_tickets (
            workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL,
            PRIMARY KEY (workspace_id, ticket_id)
         );
         INSERT INTO typed_tickets VALUES ('W', 'T');
         CREATE TABLE merge_request_schema (singleton INTEGER PRIMARY KEY, version INTEGER NOT NULL);
         INSERT INTO merge_request_schema VALUES (1, 11);
         CREATE TABLE merge_requests (
            workspace_id TEXT, merge_request_id TEXT, ticket_id TEXT, repository_id TEXT,
            state TEXT, target_ref_selector TEXT, current_revision_id TEXT,
            opened_by_worker_runtime_id TEXT, opened_by_worker_id TEXT, created_at TEXT, updated_at TEXT
         );
         CREATE TABLE merge_request_revisions (
            workspace_id TEXT, revision_id TEXT, merge_request_id TEXT, base_commit TEXT,
            head_commit TEXT, changed_paths_json TEXT, summary TEXT, assignment_id TEXT,
            coder_worker_runtime_id TEXT, coder_worker_id TEXT, created_at TEXT
         );
         CREATE TABLE merge_request_review_attempts (attempt_id TEXT);
         CREATE TABLE merge_request_reviews (
            workspace_id TEXT, review_id TEXT, revision_id TEXT,
            reviewer_worker_runtime_id TEXT, reviewer_worker_id TEXT, reviewer_profile TEXT,
            decision TEXT, body TEXT, findings_json TEXT, created_at TEXT
         );
         CREATE TABLE merge_request_completion_operations (
            workspace_id TEXT, merge_request_id TEXT, operation_id TEXT, target_commit TEXT,
            source_commit TEXT, result_commit TEXT, strategy TEXT, resolution TEXT,
            requested_by_runtime_id TEXT, requested_by_worker_id TEXT, completed_at TEXT, status TEXT
         );
         CREATE TABLE merge_request_reviewer_child_sessions (child_session_id TEXT);
         INSERT INTO merge_requests VALUES (
            'W', 'MR', 'T', 'R', 'open', 'develop', 'REV',
            'runtime', 'coder', '2026-07-26T12:00:00Z', '2026-07-26T12:00:00Z'
         );
         INSERT INTO merge_request_revisions VALUES (
            'W', 'REV', 'MR', 'base', 'head-1', '[\"src/lib.rs\"]', 'legacy', 'A1',
            'runtime', 'coder', '2026-07-26T12:00:00Z'
         );
         INSERT INTO merge_request_reviews VALUES (
            'W', 'REVIEW', 'REV', 'runtime', 'child', 'builtin:reviewer',
            'approve', 'approved', '[]', '2026-07-26T12:00:01Z'
         );",
    )
    .unwrap();

    merge_request::migrate(&conn).unwrap();
    assert_eq!(
        conn.query_row("SELECT version FROM merge_request_schema", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        12
    );
    assert_eq!(
        conn.query_row("SELECT selector_from FROM merge_requests", [], |row| row
            .get::<_, String>(
            0
        ))
        .unwrap(),
        "head-1"
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM merge_request_thread_events",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    for removed in [
        "merge_request_revisions",
        "merge_request_review_attempts",
        "merge_request_reviews",
        "merge_request_completion_operations",
    ] {
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                params![removed],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!exists, "legacy table `{removed}` still exists");
    }
}
