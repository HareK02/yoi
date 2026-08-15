use merge_request::*;
use rusqlite::{Connection, params};
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
        diff_digest: format!("sha256:diff-{head}"),
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
            target_ref_selector: "develop".into(),
            revision: revision("V1", 1, "h1"),
            authenticated_runtime_id: "R1".into(),
            authenticated_worker_id: "W1".into(),
            now: "t1".into(),
        })
        .unwrap();
}
fn attempt(store: &SqliteMergeRequestStore, id: &str, revision: &str, token: &str, child: &str) {
    store
        .register_reviewer_child_session(RegisterReviewerChildSession {
            parent_runtime_id: "R1".into(),
            parent_worker_id: "W1".into(),
            child_session_id: child.into(),
            now: "t".into(),
        })
        .unwrap();
    store
        .register_review_attempt(RegisterReviewAttempt {
            attempt_id: id.into(),
            ticket_id: "T1".into(),
            revision_id: revision.into(),
            merge_result_id: None,
            parent_assignment_id: "A1".into(),
            parent_runtime_id: "R1".into(),
            parent_worker_id: "W1".into(),
            child_session_id: child.into(),
            capability_token: token.into(),
            now: "t".into(),
        })
        .unwrap();
}
fn review(
    store: &SqliteMergeRequestStore,
    revision: &str,
    token: &str,
    decision: ReviewDecision,
) -> Result<MergeRequestReview> {
    store.submit_review(SubmitReview {
        ticket_id: "T1".into(),
        revision_id: revision.into(),
        merge_result_id: None,
        capability_token: token.into(),
        decision,
        body: "evidence".into(),
        findings: vec![],
        now: "tr".into(),
    })
}

fn record_result(
    store: &SqliteMergeRequestStore,
    revision: &str,
    target: &str,
    source: &str,
    result: &str,
    strategy: MergeStrategy,
    resolution: MergeResolution,
    operation: &str,
) -> RecordMergeResultOutcome {
    store
        .record_merge_result(RecordMergeResult {
            merge_result_id: format!("result-{operation}"),
            ticket_id: "T1".into(),
            expected_revision_id: revision.into(),
            target_commit: target.into(),
            source_commit: source.into(),
            result_commit: result.into(),
            strategy,
            resolution,
            operation_id: operation.into(),
            actor_runtime_id: "runtime-orchestrator".into(),
            actor_worker_id: "workspace-orchestrator".into(),
            created_at: "2026-07-26T00:00:02Z".into(),
        })
        .unwrap()
}

#[test]
fn storage_allows_multiple_merge_requests_for_one_ticket() {
    let (_dir, store) = setup();
    open(&store);
    store
        .open_merge_request(OpenMergeRequest {
            merge_request_id: "MR2".into(),
            ticket_id: "T1".into(),
            repository_id: "repo".into(),
            target_ref_selector: "develop".into(),
            revision: revision("V2", 1, "h2"),
            authenticated_runtime_id: "R1".into(),
            authenticated_worker_id: "W1".into(),
            now: "t2".into(),
        })
        .unwrap();
    let conn = Connection::open(store.db_path()).unwrap();
    let count:i64=conn.query_row("SELECT COUNT(*) FROM merge_request_ticket_relations WHERE workspace_id='ws-a' AND ticket_id='T1'",[],|row|row.get(0)).unwrap();
    assert_eq!(count, 2);
    assert_eq!(
        store
            .show_for_ticket("T1")
            .unwrap()
            .unwrap()
            .merge_request_id,
        "MR2"
    );
}

#[test]
fn merge_result_is_idempotent_target_fenced_and_non_ff_reviewed_independently() {
    let (_tmp, store) = setup();
    open(&store);
    attempt(
        &store,
        "attempt-source",
        "V1",
        "token-source",
        "child-source",
    );
    review(&store, "V1", "token-source", ReviewDecision::Approve).unwrap();

    let first = record_result(
        &store,
        "V1",
        "T0",
        "h1",
        "h1",
        MergeStrategy::FastForward,
        MergeResolution::None,
        "op-ff",
    );
    assert!(!first.replayed);
    let replay = record_result(
        &store,
        "V1",
        "T0",
        "h1",
        "h1",
        MergeStrategy::FastForward,
        MergeResolution::None,
        "op-ff",
    );
    assert!(replay.replayed);
    assert_eq!(
        first.merge_result.merge_result_id,
        replay.merge_result.merge_result_id
    );

    let ready = store
        .readiness_for_ticket_with_target("T1", Some("T0"))
        .unwrap();
    assert!(ready.ready, "{:?}", ready.blockers);
    assert_eq!(ready.merge_result_id.as_deref(), Some("result-op-ff"));

    let stale = store
        .readiness_for_ticket_with_target("T1", Some("T1"))
        .unwrap();
    assert!(!stale.ready);
    assert!(
        stale
            .blockers
            .iter()
            .any(|blocker| blocker.contains("target moved"))
    );

    let changed = store.record_merge_result(RecordMergeResult {
        merge_result_id: "result-conflict".into(),
        ticket_id: "T1".into(),
        expected_revision_id: "V1".into(),
        target_commit: "different".into(),
        source_commit: "h1".into(),
        result_commit: "h1".into(),
        strategy: MergeStrategy::FastForward,
        resolution: MergeResolution::None,
        operation_id: "op-ff".into(),
        actor_runtime_id: "runtime-orchestrator".into(),
        actor_worker_id: "workspace-orchestrator".into(),
        created_at: "2026-07-26T00:00:03Z".into(),
    });
    assert!(matches!(
        changed,
        Err(MergeRequestError::MergeResultOperationConflict)
    ));

    record_result(
        &store,
        "V1",
        "T1",
        "h1",
        "M1",
        MergeStrategy::Merge,
        MergeResolution::ConflictsResolved,
        "op-merge",
    );
    let pending = store
        .readiness_for_ticket_with_target("T1", Some("T1"))
        .unwrap();
    assert!(!pending.ready);
    assert_eq!(
        pending.merge_result_review_status,
        Some(ReviewStatus::Pending)
    );

    store
        .register_reviewer_child_session(RegisterReviewerChildSession {
            parent_runtime_id: "runtime-orchestrator".into(),
            parent_worker_id: "workspace-orchestrator".into(),
            child_session_id: "child-merge".into(),
            now: "2026-07-26T00:00:04Z".into(),
        })
        .unwrap();
    store
        .register_review_attempt(RegisterReviewAttempt {
            attempt_id: "attempt-merge".into(),
            ticket_id: "T1".into(),
            revision_id: "V1".into(),
            merge_result_id: Some("result-op-merge".into()),
            parent_assignment_id: "A1".into(),
            parent_runtime_id: "runtime-orchestrator".into(),
            parent_worker_id: "workspace-orchestrator".into(),
            child_session_id: "child-merge".into(),
            capability_token: "token-merge".into(),
            now: "2026-07-26T00:00:04Z".into(),
        })
        .unwrap();
    store
        .submit_review(SubmitReview {
            ticket_id: "T1".into(),
            revision_id: "V1".into(),
            merge_result_id: Some("result-op-merge".into()),
            capability_token: "token-merge".into(),
            decision: ReviewDecision::Approve,
            body: "merge evidence is valid".into(),
            findings: vec![],
            now: "2026-07-26T00:00:05Z".into(),
        })
        .unwrap();
    let approved = store
        .readiness_for_ticket_with_target("T1", Some("T1"))
        .unwrap();
    assert!(approved.ready, "{:?}", approved.blockers);
    assert_eq!(
        approved.merge_result_review_status,
        Some(ReviewStatus::Approved)
    );
    let applied = store
        .show_for_ticket_with_target("T1", Some("M1"))
        .unwrap()
        .unwrap();
    assert_eq!(
        applied
            .applied_merge_result
            .as_ref()
            .map(|result| result.target_status),
        Some(MergeResultTargetStatus::Applied)
    );
    assert!(
        store
            .readiness_for_ticket_with_target("T1", Some("M1"))
            .unwrap()
            .ready
    );
}

#[test]
fn bounded_context_rejects_oversized_revision_evidence() {
    let (_dir, store) = setup();
    let mut oversized = revision("V1", 1, "h1");
    oversized.changed_paths = (0..=1_000).map(|i| format!("src/{i}.rs")).collect();
    let result = store.open_merge_request(OpenMergeRequest {
        merge_request_id: "MR1".into(),
        ticket_id: "T1".into(),
        repository_id: "repo".into(),
        target_ref_selector: "develop".into(),
        revision: oversized,
        authenticated_runtime_id: "R1".into(),
        authenticated_worker_id: "W1".into(),
        now: "t".into(),
    });
    assert!(matches!(
        result,
        Err(MergeRequestError::TooLarge {
            field: "revision.changed_paths",
            ..
        })
    ));
}

#[test]
fn rejected_v6_schema_missing_diff_digest_is_archived_before_fresh_v7() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE merge_request_schema_migrations(version INTEGER PRIMARY KEY,name TEXT NOT NULL,applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);\
         INSERT INTO merge_request_schema_migrations(version,name) VALUES(6,'rejected_merge_request_v6');\
         CREATE TABLE repositories(workspace_id TEXT NOT NULL,repository_id TEXT NOT NULL,PRIMARY KEY(workspace_id,repository_id));\
         CREATE TABLE typed_tickets(workspace_id TEXT NOT NULL,ticket_id TEXT NOT NULL,workflow_state TEXT NOT NULL,workflow_state_explicit INTEGER NOT NULL DEFAULT 1,updated_at TEXT NOT NULL,PRIMARY KEY(workspace_id,ticket_id));\
         CREATE TABLE ticket_worker_assignments(workspace_id TEXT NOT NULL,ticket_id TEXT NOT NULL,assignment_id TEXT NOT NULL,runtime_id TEXT NOT NULL,worker_id TEXT NOT NULL,PRIMARY KEY(workspace_id,ticket_id,assignment_id));\
         CREATE TABLE merge_requests(workspace_id TEXT NOT NULL,merge_request_id TEXT NOT NULL,repository_id TEXT NOT NULL,state TEXT NOT NULL,lifecycle_generation INTEGER NOT NULL,current_revision_id TEXT NOT NULL,created_at TEXT NOT NULL,updated_at TEXT NOT NULL,PRIMARY KEY(workspace_id,merge_request_id));\
         CREATE TABLE merge_request_ticket_relations(workspace_id TEXT NOT NULL,merge_request_id TEXT NOT NULL,ticket_id TEXT NOT NULL,relation_kind TEXT NOT NULL,created_at TEXT NOT NULL,PRIMARY KEY(workspace_id,merge_request_id,ticket_id));\
         CREATE TABLE merge_request_revisions(workspace_id TEXT NOT NULL,merge_request_id TEXT NOT NULL,revision_id TEXT NOT NULL,ordinal INTEGER NOT NULL,base_commit TEXT NOT NULL,head_commit TEXT NOT NULL,head_tree TEXT NOT NULL,assignment_id TEXT NOT NULL,created_at TEXT NOT NULL,PRIMARY KEY(workspace_id,merge_request_id,revision_id));",
    ).unwrap();
    drop(conn);
    let store = SqliteMergeRequestStore::open(&path, "ws-a").unwrap();
    assert!(store.show_for_ticket("missing").unwrap().is_none());
    let conn = Connection::open(&path).unwrap();
    let archived: i64 = conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='legacy_v6_merge_requests'",[],|row|row.get(0)).unwrap();
    assert_eq!(archived, 1);
    for table in [
        "merge_request_review_attempts",
        "merge_request_completion_operations",
    ] {
        let present: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                params![table],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(present, 1);
    }
}

#[test]
fn interrupted_legacy_archive_with_empty_recreated_table_resumes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("interrupted.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE merge_request_schema_migrations(version INTEGER PRIMARY KEY,name TEXT NOT NULL,applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);\
         INSERT INTO merge_request_schema_migrations(version,name) VALUES(6,'rejected_merge_request_v6');\
         CREATE TABLE merge_requests(workspace_id TEXT NOT NULL,merge_request_id TEXT NOT NULL,ticket_id TEXT NOT NULL);\
         CREATE TABLE merge_request_review_findings(workspace_id TEXT NOT NULL,attempt_id TEXT NOT NULL,ordinal INTEGER NOT NULL,severity TEXT NOT NULL,code TEXT,path TEXT,line INTEGER,body TEXT NOT NULL);\
         CREATE TABLE legacy_v6_merge_request_review_findings(workspace_id TEXT NOT NULL,attempt_id TEXT NOT NULL,ordinal INTEGER NOT NULL,severity TEXT NOT NULL,code TEXT,path TEXT,line INTEGER,body TEXT NOT NULL);\
         INSERT INTO legacy_v6_merge_request_review_findings VALUES('ws-a','AT1',0,'warning',NULL,NULL,NULL,'preserved evidence');",
    )
    .unwrap();
    drop(conn);

    SqliteMergeRequestStore::open(&path, "ws-a").unwrap();
    let conn = Connection::open(&path).unwrap();
    let archived_body: String = conn
        .query_row(
            "SELECT body FROM legacy_v6_merge_request_review_findings",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(archived_body, "preserved evidence");
    let version: i64 = conn
        .query_row(
            "SELECT MAX(version) FROM merge_request_schema_migrations",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, 8);
    drop(conn);

    SqliteMergeRequestStore::open(&path, "ws-a").unwrap();
}

#[test]
fn conflicting_legacy_archive_rolls_back_all_table_renames() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("conflict.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE merge_request_schema_migrations(version INTEGER PRIMARY KEY,name TEXT NOT NULL,applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);\
         INSERT INTO merge_request_schema_migrations(version,name) VALUES(6,'rejected_merge_request_v6');\
         CREATE TABLE merge_requests(workspace_id TEXT NOT NULL,merge_request_id TEXT NOT NULL,ticket_id TEXT NOT NULL);\
         CREATE TABLE merge_request_review_findings(body TEXT NOT NULL);\
         CREATE TABLE merge_request_reviews(body TEXT NOT NULL);\
         INSERT INTO merge_request_reviews VALUES('unarchived evidence');\
         CREATE TABLE legacy_v6_merge_request_reviews(body TEXT NOT NULL);",
    )
    .unwrap();

    let error = migrate(&conn).unwrap_err();
    assert!(error.to_string().contains(
        "legacy archive table legacy_v6_merge_request_reviews already exists while merge_request_reviews still contains data"
    ));
    let current_findings: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='merge_request_review_findings'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let archived_findings: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='legacy_v6_merge_request_review_findings'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(current_findings, 1);
    assert_eq!(archived_findings, 0);
}

#[test]
fn v7_completion_operations_are_preserved_as_legacy_assigned_coder_authority() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v7.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE merge_request_schema_migrations(version INTEGER PRIMARY KEY,name TEXT NOT NULL,applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);\
         INSERT INTO merge_request_schema_migrations(version,name) VALUES(7,'fresh_bounded_context_authority');\
         CREATE TABLE repositories(workspace_id TEXT NOT NULL,repository_id TEXT NOT NULL,PRIMARY KEY(workspace_id,repository_id));\
         CREATE TABLE typed_tickets(workspace_id TEXT NOT NULL,ticket_id TEXT NOT NULL,workflow_state TEXT NOT NULL,workflow_state_explicit INTEGER NOT NULL DEFAULT 1,updated_at TEXT NOT NULL,PRIMARY KEY(workspace_id,ticket_id));\
         INSERT INTO typed_tickets VALUES('ws-a','T1','done',1,'t');\
         CREATE TABLE merge_request_completion_operations(workspace_id TEXT NOT NULL,operation_id TEXT NOT NULL,ticket_id TEXT NOT NULL,revision_id TEXT NOT NULL,assignment_id TEXT NOT NULL,fingerprint TEXT NOT NULL,status TEXT NOT NULL CHECK(status IN ('pending','completed')),result_ticket_state TEXT,created_at TEXT NOT NULL,updated_at TEXT NOT NULL,PRIMARY KEY(workspace_id,operation_id),FOREIGN KEY(workspace_id,ticket_id) REFERENCES typed_tickets(workspace_id,ticket_id));\
         INSERT INTO merge_request_completion_operations VALUES('ws-a','legacy-op','T1','V1','A1','legacy-fingerprint','completed','done','t','t');",
    ).unwrap();
    drop(conn);

    SqliteMergeRequestStore::open(&path, "ws-a").unwrap();
    let conn = Connection::open(&path).unwrap();
    let row: (String, String, Option<String>, Option<String>, String) = conn
        .query_row(
            "SELECT authority_kind,implementation_assignment_id,completion_actor_runtime_id,completion_actor_worker_id,fingerprint FROM merge_request_completion_operations WHERE operation_id='legacy-op'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .unwrap();
    assert_eq!(
        row,
        (
            "legacy_assigned_coder".into(),
            "A1".into(),
            None,
            None,
            "legacy-fingerprint".into()
        )
    );
    let version: i64 = conn
        .query_row(
            "SELECT MAX(version) FROM merge_request_schema_migrations",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, 9);
    let head_tree_columns: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('merge_request_revisions') WHERE name='head_tree'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(head_tree_columns, 0);
    let merge_result_table: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='merge_request_merge_results'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(merge_result_table, 1);
}

#[test]
fn request_changes_new_revision_resets_and_exact_completion_replay_converges() {
    let (_dir, store) = setup();
    open(&store);
    attempt(&store, "AT1", "V1", "tok1", "child1");
    review(&store, "V1", "tok1", ReviewDecision::RequestChanges).unwrap();
    assert_eq!(
        store.show_for_ticket("T1").unwrap().unwrap().review_status,
        ReviewStatus::ChangesRequested
    );
    store
        .add_revision(AddRevision {
            ticket_id: "T1".into(),
            expected_current_revision_id: "V1".into(),
            revision: revision("V2", 2, "h2"),
            authenticated_runtime_id: "R1".into(),
            authenticated_worker_id: "W1".into(),
            now: "t2".into(),
        })
        .unwrap();
    assert_eq!(
        store.show_for_ticket("T1").unwrap().unwrap().review_status,
        ReviewStatus::Pending
    );
    assert!(review(&store, "V1", "tok1", ReviewDecision::Approve).is_err());
    attempt(&store, "AT2", "V2", "tok2", "child2");
    review(&store, "V2", "tok2", ReviewDecision::Approve).unwrap();
    let input = CompleteMergeRequest {
        operation_id: "OP1".into(),
        ticket_id: "T1".into(),
        expected_revision_id: "V2".into(),
        implementation_assignment_id: "A1".into(),
        completion_actor_runtime_id: "OR".into(),
        completion_actor_worker_id: "OW".into(),
        now: "tc".into(),
    };
    let first = store.complete(input.clone()).unwrap();
    assert!(!first.replayed);
    let replay = store.complete(input).unwrap();
    assert!(replay.replayed);
    assert!(matches!(
        store.confirm_merge(MergeConfirmation {
            ticket_id: "T1".into(),
            expected_revision_id: "V2".into(),
            authenticated_account_id: "runtime".into(),
            actor_kind: "worker".into(),
            explicit_confirmation: true,
            now: "tm".into()
        }),
        Err(MergeRequestError::MergeConfirmationRequired)
    ));
    let merged = store
        .confirm_merge(MergeConfirmation {
            ticket_id: "T1".into(),
            expected_revision_id: "V2".into(),
            authenticated_account_id: "account-1".into(),
            actor_kind: "user".into(),
            explicit_confirmation: true,
            now: "tm".into(),
        })
        .unwrap();
    assert_eq!(merged.state, MergeRequestState::Merged);
    let conn = Connection::open(store.db_path()).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT workflow_state FROM typed_tickets WHERE workspace_id='ws-a' AND ticket_id='T1'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "done"
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM typed_ticket_events WHERE workspace_id='ws-a' AND ticket_id='T1'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row(
            "SELECT authority_kind || ':' || implementation_assignment_id || ':' || completion_actor_runtime_id || ':' || completion_actor_worker_id FROM merge_request_completion_operations WHERE workspace_id='ws-a' AND operation_id='OP1'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "workspace_orchestrator:A1:OR:OW"
    );
    assert_eq!(
        conn.query_row(
            "SELECT author FROM typed_ticket_events WHERE workspace_id='ws-a' AND ticket_id='T1' AND kind='state_changed'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "worker:OR:OW"
    );
    let authority: String = conn
        .query_row(
            "SELECT value FROM typed_ticket_event_attributes WHERE workspace_id='ws-a' AND ticket_id='T1' AND key='completion_authority'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(authority, "workspace_orchestrator");
}

#[test]
fn spoof_self_approval_replay_and_cross_workspace_are_rejected() {
    let (_dir, store) = setup();
    open(&store);
    let mut bad = RegisterReviewAttempt {
        attempt_id: "bad".into(),
        ticket_id: "T1".into(),
        revision_id: "V1".into(),
        merge_result_id: None,
        parent_assignment_id: "A1".into(),
        parent_runtime_id: "R1".into(),
        parent_worker_id: "W1".into(),
        child_session_id: "W1".into(),
        capability_token: "bad".into(),
        now: "t".into(),
    };
    assert!(matches!(
        store.register_review_attempt(bad.clone()),
        Err(MergeRequestError::SelfApproval)
    ));
    bad.child_session_id = "child".into();
    assert!(matches!(
        store.register_review_attempt(bad),
        Err(MergeRequestError::InvalidReviewer)
    ));
    attempt(&store, "AT", "V1", "secret", "child");
    assert!(review(&store, "V1", "spoof", ReviewDecision::Approve).is_err());
    review(&store, "V1", "secret", ReviewDecision::Approve).unwrap();
    assert!(review(&store, "V1", "secret", ReviewDecision::Approve).is_err());
    let other = SqliteMergeRequestStore::open_verified(store.db_path(), "ws-b").unwrap();
    assert!(other.show_for_ticket("T1").unwrap().is_none());
}

#[test]
fn reopen_resets_approval_and_merge_requires_authenticated_explicit_user() {
    let (_dir, store) = setup();
    open(&store);
    attempt(&store, "AT", "V1", "token", "child");
    review(&store, "V1", "token", ReviewDecision::Approve).unwrap();
    store.close("T1", "V1", "tc").unwrap();
    let reopened = store.reopen("T1", "V1", "tr").unwrap();
    assert_eq!(reopened.review_status, ReviewStatus::Pending);
    let denied = store.confirm_merge(MergeConfirmation {
        ticket_id: "T1".into(),
        expected_revision_id: "V1".into(),
        authenticated_account_id: "user".into(),
        actor_kind: "user".into(),
        explicit_confirmation: false,
        now: "tm".into(),
    });
    assert!(matches!(
        denied,
        Err(MergeRequestError::MergeConfirmationRequired)
    ));
}

#[test]
fn concurrent_exact_completion_replays_commit_one_ticket_side_effect() {
    let (_dir, store) = setup();
    open(&store);
    attempt(&store, "AT", "V1", "token", "child");
    review(&store, "V1", "token", ReviewDecision::Approve).unwrap();
    let input = CompleteMergeRequest {
        operation_id: "OP-concurrent".into(),
        ticket_id: "T1".into(),
        expected_revision_id: "V1".into(),
        implementation_assignment_id: "A1".into(),
        completion_actor_runtime_id: "OR".into(),
        completion_actor_worker_id: "OW".into(),
        now: "t".into(),
    };
    let left_store = store.clone();
    let left_input = input.clone();
    let left = std::thread::spawn(move || left_store.complete(left_input));
    let right_store = store.clone();
    let right = std::thread::spawn(move || right_store.complete(input));
    let outcomes = [
        left.join().unwrap().unwrap(),
        right.join().unwrap().unwrap(),
    ];
    assert_eq!(
        outcomes.iter().filter(|outcome| !outcome.replayed).count(),
        1
    );
    assert_eq!(
        outcomes.iter().filter(|outcome| outcome.replayed).count(),
        1
    );
    let conn = Connection::open(store.db_path()).unwrap();
    let events: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM typed_ticket_events WHERE workspace_id='ws-a' AND ticket_id='T1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(events, 1);
}

#[test]
fn operation_key_mismatch_and_actor_or_assignment_change_are_fenced() {
    let (_dir, store) = setup();
    open(&store);
    attempt(&store, "AT", "V1", "token", "child");
    review(&store, "V1", "token", ReviewDecision::Approve).unwrap();
    let mut input = CompleteMergeRequest {
        operation_id: "OP".into(),
        ticket_id: "T1".into(),
        expected_revision_id: "V1".into(),
        implementation_assignment_id: "A1".into(),
        completion_actor_runtime_id: "OR".into(),
        completion_actor_worker_id: "OW".into(),
        now: "t".into(),
    };
    let conn = Connection::open(store.db_path()).unwrap();
    conn.execute(
        "UPDATE ticket_current_worker_assignments SET assignment_id='A2',runtime_id='R2',worker_id='W2' WHERE workspace_id='ws-a' AND ticket_id='T1'",
        [],
    )
    .unwrap();
    assert!(matches!(
        store.complete(input.clone()),
        Err(MergeRequestError::AssignmentMismatch)
    ));
    conn.execute(
        "UPDATE ticket_current_worker_assignments SET assignment_id='A1',runtime_id='R1',worker_id='W1' WHERE workspace_id='ws-a' AND ticket_id='T1'",
        [],
    )
    .unwrap();
    store.complete(input.clone()).unwrap();
    input.completion_actor_worker_id = "other".into();
    assert!(matches!(
        store.complete(input.clone()),
        Err(MergeRequestError::OperationConflict)
    ));
    input.completion_actor_worker_id = "OW".into();
    input.implementation_assignment_id = "A2".into();
    assert!(matches!(
        store.complete(input.clone()),
        Err(MergeRequestError::OperationConflict)
    ));
    input.implementation_assignment_id = "A1".into();
    input.expected_revision_id = "other".into();
    assert!(matches!(
        store.complete(input),
        Err(MergeRequestError::OperationConflict)
    ));
}
