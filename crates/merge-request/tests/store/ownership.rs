//! Ownership component tests use the real store and temporary SQLite. Ticket
//! assignment and repository admission are fixture boundaries, not live services.
use super::*;

fn worker(name: &str) -> WorkerIdentity {
    WorkerIdentity {
        runtime_id: "runtime".into(),
        worker_id: name.into(),
    }
}

pub(super) fn transfer(
    store: &MergeRequestStore,
    operation: &str,
    desired: Option<&str>,
) -> SetMergeRequestWorker {
    SetMergeRequestWorker {
        workspace_id: "W".into(),
        merge_request_id: "MR".into(),
        operation_id: operation.into(),
        expected_operation_id: store
            .current_worker("W", "MR")
            .unwrap()
            .map(|e| e.operation_id),
        worker: desired.map(worker),
        assigned_by: worker("coordinator"),
        now: at(6),
    }
}

fn review_request(auth: MergeRequestAuth, token: &str) -> RequestMergeRequestReview {
    RequestMergeRequestReview {
        merge_request_id: "MR".into(),
        ticket_id: "T".into(),
        ticket_content_digest: content_digest("Title"),
        ticket_merge_request_subjects: vec![MergeRequestReviewSubject {
            merge_request_id: "MR".into(),
            subject_ref: "source".into(),
        }],
        subject_ref: "source".into(),
        child_session_id: format!("child-{token}"),
        capability_token: token.into(),
        auth,
        now: at(7),
    }
}

fn child(store: &MergeRequestStore, parent: &str, token: &str) {
    store
        .register_reviewer_child_session(RegisterReviewerChildSession {
            workspace_id: "W".into(),
            parent_runtime_id: "runtime".into(),
            parent_worker_id: parent.into(),
            child_session_id: format!("child-{token}"),
            reviewer_profile: "builtin:reviewer".into(),
            now: at(6),
        })
        .unwrap();
}

#[test]
fn open_binds_opener_atomically_even_without_summary_and_rolls_back_on_evidence_failure() {
    for summary in ["", "summary"] {
        let (dir, store) = fixture();
        let input = OpenMergeRequest {
            merge_request_id: "MR".into(),
            ticket_id: "T".into(),
            repository_id: "R".into(),
            selector_from: "work/source".into(),
            selector_to: "main".into(),
            summary: summary.into(),
            auth: auth(),
            now: at(1),
        };
        assert!(store.open_merge_request_with_evidence(input.clone(), |transaction| {
            let owner: String = transaction.query_row(
                "SELECT json_extract(payload_json,'$.worker.worker_id') FROM merge_request_thread_events WHERE kind='worker_assigned'", [], |r| r.get(0),
            )?;
            assert_eq!(owner, "coder");
            Err(MergeRequestError::Operation("injected evidence failure".into()))
        }).is_err());
        assert!(matches!(
            store.current_worker("W", "MR"),
            Err(MergeRequestError::NotFound)
        ));
        let c = Connection::open(dir.path().join("db")).unwrap();
        for table in [
            "merge_requests",
            "merge_request_ticket_relations",
            "merge_request_thread_events",
        ] {
            assert_eq!(
                c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
        let mr = store.open_merge_request(input).unwrap();
        let owner = store.current_worker("W", "MR").unwrap().unwrap();
        assert_eq!(owner.worker, Some(worker("coder")));
        assert_eq!(owner.assigned_by, worker("coder"));
        assert_eq!(owner.previous_operation_id, None);
        assert!(!owner.operation_id.is_empty());
        assert_ne!(owner.operation_id, auth().assignment_id);
        assert_ne!(owner.operation_id, owner.event_id);
        assert_eq!(mr.current_worker(), Some(&owner));
        assert_eq!(
            mr.thread.first(),
            Some(&MergeRequestThreadEvent::WorkerAssigned(owner.clone()))
        );
        let json = serde_json::to_value(&mr.thread[0]).unwrap();
        assert_eq!(json["kind"], "worker_assigned");
        assert!(json["previous_operation_id"].is_null());
        assert_eq!(
            serde_json::from_value::<MergeRequestThreadEvent>(json).unwrap(),
            mr.thread[0]
        );
    }
}

#[test]
fn transfer_cas_fences_aba_and_release_while_exact_replay_preserves_original_receipt() {
    let (_dir, store) = fixture();
    open(&store);
    let initial = store.current_worker("W", "MR").unwrap().unwrap();
    let to_b = transfer(&store, "to-b", Some("b"));
    let stale = transfer(&store, "stale", Some("stale"));
    let receipt_b = store.set_worker(to_b.clone(), |_| Ok(())).unwrap();
    let to_a = transfer(&store, "to-a", Some("coder"));
    let receipt_a = store.set_worker(to_a, |_| Ok(())).unwrap();
    assert_eq!(receipt_a.worker, initial.worker);
    assert_ne!(receipt_a.operation_id, initial.operation_id);
    assert!(matches!(
        store.set_worker(stale, |_| Ok(())),
        Err(MergeRequestError::Conflict(_))
    ));
    let mut retry = to_b.clone();
    retry.now = at(59);
    assert_eq!(store.set_worker(retry, |_| Ok(())).unwrap(), receipt_b);
    assert_eq!(store.current_worker("W", "MR").unwrap(), Some(receipt_a));
    for changed in [
        SetMergeRequestWorker {
            worker: None,
            ..to_b.clone()
        },
        SetMergeRequestWorker {
            assigned_by: worker("other-actor"),
            ..to_b.clone()
        },
        SetMergeRequestWorker {
            expected_operation_id: None,
            ..to_b.clone()
        },
    ] {
        assert!(matches!(
            store.set_worker(changed, |_| Ok(())),
            Err(MergeRequestError::Conflict(_))
        ));
    }
    let release = transfer(&store, "release", None);
    let receipt = store.set_worker(release.clone(), |_| Ok(())).unwrap();
    assert_eq!(receipt.worker, None);
    assert!(matches!(
        store.request_review(review_request(mr_auth(&store, "MR", "R"), "released")),
        Err(MergeRequestError::Unauthorized(_))
    ));
    assert_eq!(store.set_worker(release, |_| Ok(())).unwrap(), receipt);
    let mut null_aba = transfer(&store, "null-aba", Some("coder"));
    null_aba.expected_operation_id = None;
    assert!(matches!(
        store.set_worker(null_aba, |_| Ok(())),
        Err(MergeRequestError::Conflict(_))
    ));
    let reassign = transfer(&store, "reassign", Some("coder"));
    store.set_worker(reassign, |_| Ok(())).unwrap();
    let same_worker = transfer(&store, "same-worker", Some("coder"));
    let same = store.set_worker(same_worker, |_| Ok(())).unwrap();
    assert_eq!(same.previous_operation_id.as_deref(), Some("reassign"));
    assert_eq!(store.current_worker("W", "MR").unwrap(), Some(same));
    open_for(&store, "MR-2", "R2");
    let cross_mr = SetMergeRequestWorker {
        merge_request_id: "MR-2".into(),
        ..to_b
    };
    assert!(matches!(
        store.set_worker(cross_mr, |_| Ok(())),
        Err(MergeRequestError::Conflict(_))
    ));
}

#[test]
fn admission_denial_and_storage_failure_do_not_change_owner_and_replay_rechecks_permission() {
    let (dir, store) = fixture();
    open(&store);
    let before = store.get_by_id("W", "MR").unwrap();
    let input = transfer(&store, "change", Some("b"));
    let deny = |connection: &Connection| {
        assert!(!connection.is_autocommit());
        Err(MergeRequestError::Unauthorized(
            "Host denied actor or target resources".into(),
        ))
    };
    assert!(matches!(
        store.set_worker(input.clone(), deny),
        Err(MergeRequestError::Unauthorized(_))
    ));
    assert_eq!(store.get_by_id("W", "MR").unwrap(), before);
    let c = Connection::open(dir.path().join("db")).unwrap();
    c.execute_batch("CREATE TRIGGER fail_owner_update BEFORE UPDATE OF updated_at ON merge_requests BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(store.set_worker(input.clone(), |_| Ok(())).is_err());
    assert_eq!(store.get_by_id("W", "MR").unwrap(), before);
    c.execute_batch("DROP TRIGGER fail_owner_update").unwrap();
    let saved = store.set_worker(input.clone(), |_| Ok(())).unwrap();
    assert!(matches!(
        store.set_worker(input.clone(), deny),
        Err(MergeRequestError::Unauthorized(_))
    ));
    assert_eq!(store.set_worker(input, |_| Ok(())).unwrap(), saved);
    assert_eq!(
        store.get_by_id("W", "MR").unwrap().thread.len(),
        before.thread.len() + 1
    );
}

#[test]
fn owner_continues_through_integration_and_transfer_preserves_pending_and_effective_reviews() {
    let (_dir, store) = fixture();
    open(&store);
    let original_owner = store.current_worker("W", "MR").unwrap();
    let review = approve(&store, "source", "approved");
    assert_eq!(store.current_worker("W", "MR").unwrap(), original_owner);
    request(&store, "source", "pending");
    let before = store.get_by_id("W", "MR").unwrap();
    let input = transfer(&store, "transfer", Some("b"));
    let event = store.set_worker(input, |_| Ok(())).unwrap();
    let after = store.get_by_id("W", "MR").unwrap();
    assert_eq!(before.state, after.state);
    assert_eq!(after.phase(), MergeRequestPhase::Integration);
    assert_eq!(after.effective_review("source"), Some(&review));
    assert_eq!(&after.thread[..before.thread.len()], &before.thread);
    assert_eq!(
        after.thread.last(),
        Some(&MergeRequestThreadEvent::WorkerAssigned(event))
    );
    store.authorize_review_submission("MR", "pending").unwrap();
    let pending = store
        .submit_review(SubmitMergeRequestReview {
            merge_request_id: "MR".into(),
            ticket_id: "T".into(),
            current_subject_ref: "source".into(),
            capability_token: "pending".into(),
            decision: ReviewDecision::Approve,
            body: "approved".into(),
            findings: vec![],
            now: at(9),
        })
        .unwrap();
    assert_eq!(pending.reviewer, worker("child-pending"));
    assert_eq!(store.get_by_id("W", "MR").unwrap().state, before.state);
}

#[test]
fn review_actions_reject_stale_identity_and_operation_even_after_aba_without_ticket_dependence() {
    struct NoTicketAssignments;
    impl AssignmentSource for NoTicketAssignments {
        fn current_assignment(
            &self,
            _: &str,
            _: &str,
        ) -> Result<Option<CurrentAssignment>, String> {
            Err("Ticket assignment must not be consulted after open".into())
        }
    }
    let (dir, original) = fixture();
    open(&original);
    let old_auth = mr_auth(&original, "MR", "R");
    let review = approve(&original, "source", "initial");
    let store = MergeRequestStore::open(
        dir.path().join("db"),
        Arc::new(NoTicketAssignments),
        Arc::new(Repositories),
    )
    .unwrap();
    Connection::open(dir.path().join("db"))
        .unwrap()
        .execute_batch("DELETE FROM ticket_current_worker_assignments;")
        .unwrap();
    let to_b = transfer(&store, "b", Some("b"));
    store.set_worker(to_b, |_| Ok(())).unwrap();
    let b_auth = MergeRequestAuth {
        worker_id: "b".into(),
        ..mr_auth(&store, "MR", "R")
    };
    child(&store, "b", "transferred");
    store
        .request_review(review_request(b_auth.clone(), "transferred"))
        .unwrap();
    let to_a = transfer(&store, "back", Some("coder"));
    store.set_worker(to_a, |_| Ok(())).unwrap();
    let current_auth = mr_auth(&store, "MR", "R");
    let wrong_identity = MergeRequestAuth {
        worker_id: "b".into(),
        ..current_auth.clone()
    };
    let wrong_runtime = MergeRequestAuth {
        runtime_id: "other".into(),
        ..current_auth.clone()
    };
    for (index, stale) in [old_auth, b_auth, wrong_identity, wrong_runtime, auth()]
        .into_iter()
        .enumerate()
    {
        let before = store.get_by_id("W", "MR").unwrap();
        assert!(matches!(
            store.request_review(review_request(stale.clone(), &format!("stale-{index}"))),
            Err(MergeRequestError::Unauthorized(_))
        ));
        assert!(matches!(
            store.revoke_review(RevokeMergeRequestReview {
                merge_request_id: "MR".into(),
                ticket_id: "T".into(),
                review_event_id: review.event_id.clone(),
                reason: "stale".into(),
                auth: stale,
                now: at(8),
            }),
            Err(MergeRequestError::Unauthorized(_))
        ));
        assert_eq!(store.get_by_id("W", "MR").unwrap(), before);
    }
    child(&store, "coder", "current");
    store
        .request_review(review_request(current_auth.clone(), "current"))
        .unwrap();
    store
        .revoke_review(RevokeMergeRequestReview {
            merge_request_id: "MR".into(),
            ticket_id: "T".into(),
            review_event_id: review.event_id,
            reason: "current owner".into(),
            auth: current_auth,
            now: at(8),
        })
        .unwrap();
    // Independent Reviewer attestation is still required, not granted by ownership.
    child(&store, "other-parent", "wrong-parent");
    assert!(matches!(
        store.request_review(review_request(mr_auth(&store, "MR", "R"), "wrong-parent")),
        Err(MergeRequestError::Unauthorized(_))
    ));
}

#[test]
fn concurrent_transfers_using_same_observation_commit_only_one_owner() {
    let (dir, store) = fixture();
    open(&store);
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let joins: Vec<_> = ["left", "right"]
        .into_iter()
        .map(|name| {
            let input = transfer(&store, name, Some(name));
            let barrier = barrier.clone();
            let path = dir.path().join("db");
            std::thread::spawn(move || {
                let assignments = Assignments(Arc::new(Mutex::new(CurrentAssignment {
                    assignment_id: "unused".into(),
                    ticket_id: "T".into(),
                    runtime_id: "runtime".into(),
                    worker_id: "unused".into(),
                })));
                let store =
                    MergeRequestStore::open(path, Arc::new(assignments), Arc::new(Repositories))
                        .unwrap();
                barrier.wait();
                store.set_worker(input, |_| Ok(()))
            })
        })
        .collect();
    let results: Vec<_> = joins.into_iter().map(|join| join.join().unwrap()).collect();
    assert_eq!(
        results.iter().filter(|r| r.is_ok()).count(),
        1,
        "{results:?}"
    );
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(MergeRequestError::Conflict(_))))
            .count(),
        1,
        "{results:?}"
    );
    let winner = results.into_iter().find_map(Result::ok).unwrap();
    assert_eq!(store.current_worker("W", "MR").unwrap(), Some(winner));
}

// Freeze the pre-ownership event constraint and rows, not merely the version.
pub(super) fn freeze_v14_schema(c: &Connection) {
    c.execute_batch("DROP TABLE merge_request_publications;")
        .unwrap();
    let triggers: Vec<(String, String)> = c.prepare(
        "SELECT name,sql FROM sqlite_master WHERE type='trigger' AND name LIKE 'merge_request_lifecycle_%' ORDER BY name",
    ).unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
    for (name, _) in &triggers {
        c.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();
    }
    c.execute_batch("
        CREATE TEMP TABLE frozen_grants AS SELECT * FROM merge_request_review_grants;
        CREATE TEMP TABLE frozen_events AS SELECT * FROM merge_request_thread_events WHERE kind != 'worker_assigned';
        DROP TABLE merge_request_review_grants;
        DROP TABLE merge_request_thread_events;
        CREATE TABLE merge_request_thread_events(workspace_id TEXT NOT NULL,merge_request_id TEXT NOT NULL,event_id TEXT NOT NULL,sequence INTEGER NOT NULL,kind TEXT NOT NULL CHECK(kind IN('review_requested','review','review_revoked','review_cancelled','comment','merge')),payload_json TEXT NOT NULL,operation_id TEXT,created_at TEXT NOT NULL,PRIMARY KEY(workspace_id,merge_request_id,event_id),UNIQUE(workspace_id,merge_request_id,sequence),FOREIGN KEY(workspace_id,merge_request_id)REFERENCES merge_requests(workspace_id,merge_request_id)ON DELETE CASCADE);
        CREATE UNIQUE INDEX merge_request_merge_operations ON merge_request_thread_events(workspace_id,operation_id)WHERE operation_id IS NOT NULL;
        CREATE TABLE merge_request_review_grants(workspace_id TEXT NOT NULL,merge_request_id TEXT NOT NULL,request_event_id TEXT NOT NULL,subject_ref TEXT NOT NULL,reviewer_runtime_id TEXT NOT NULL,reviewer_worker_id TEXT NOT NULL,capability_token TEXT PRIMARY KEY,issued_at TEXT NOT NULL,consumed_at TEXT,revoked_at TEXT,status TEXT NOT NULL CHECK(status IN('issued','consumed','revoked')),FOREIGN KEY(workspace_id,merge_request_id,request_event_id)REFERENCES merge_request_thread_events(workspace_id,merge_request_id,event_id)ON DELETE CASCADE);
        INSERT INTO merge_request_thread_events SELECT * FROM frozen_events;
        INSERT INTO merge_request_review_grants SELECT * FROM frozen_grants;
        DROP TABLE frozen_events;
        DROP TABLE frozen_grants;
        UPDATE merge_request_schema SET version=14;
    ").unwrap();
    for (_, sql) in triggers {
        c.execute_batch(&sql).unwrap();
    }
}

#[test]
fn v14_migration_preserves_reviews_and_phase_without_fabricating_ticket_owner_and_rolls_back() {
    let (dir, store) = fixture();
    open(&store);
    let review = approve(&store, "source", "approved");
    request(&store, "source", "issued");
    let c = Connection::open(dir.path().join("db")).unwrap();
    c.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    freeze_v14_schema(&c);
    c.execute_batch("CREATE TABLE notification_probe(event_id TEXT); CREATE TRIGGER notification_probe_event AFTER INSERT ON merge_request_thread_events BEGIN INSERT INTO notification_probe VALUES(NEW.event_id); END;").unwrap();
    let trigger_sql: String = c
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='notification_probe_event'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let before = store.get_by_id("W", "MR").unwrap();
    assert!(before.current_worker().is_none());
    let schema = |c: &Connection| {
        c.prepare("SELECT type,name,sql FROM sqlite_master WHERE name LIKE 'merge_request%' ORDER BY type,name").unwrap()
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?))).unwrap()
            .collect::<Result<Vec<_>, _>>().unwrap()
    };
    let old_schema = schema(&c);
    c.execute_batch("CREATE TRIGGER fail_ownership_migration BEFORE UPDATE ON merge_request_schema BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(migrate(&c).is_err());
    assert_eq!(schema(&c), old_schema);
    assert_eq!(store.get_by_id("W", "MR").unwrap(), before);
    assert_eq!(
        c.query_row("SELECT version FROM merge_request_schema", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        14
    );
    store.authorize_review_submission("MR", "issued").unwrap();
    c.execute_batch("DROP TRIGGER fail_ownership_migration;")
        .unwrap();
    migrate(&c).unwrap();
    migrate(&c).unwrap();
    assert_eq!(
        c.query_row(
            "SELECT sql FROM sqlite_master WHERE name='notification_probe_event'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        trigger_sql
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM notification_probe", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0,
        "migration must not redeliver historical events"
    );
    assert_eq!(store.get_by_id("W", "MR").unwrap(), before);
    assert_eq!(store.current_worker("W", "MR").unwrap(), None);
    assert_eq!(before.effective_review("source"), Some(&review));
    store.authorize_review_submission("MR", "issued").unwrap();
    assert!(
        !c.prepare("PRAGMA foreign_key_check")
            .unwrap()
            .exists([])
            .unwrap()
    );
    assert!(
        matches!(store.request_review(review_request(auth(), "legacy")), Err(MergeRequestError::Unauthorized(message)) if message.contains("legacy"))
    );
    let explicit = transfer(&store, "legacy-adoption", Some("coder"));
    assert_eq!(explicit.expected_operation_id, None);
    store.set_worker(explicit, |_| Ok(())).unwrap();
    request(&store, "source", "new-owner");
    assert_eq!(
        c.query_row("SELECT count(*) FROM notification_probe", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2,
        "future assignment and review events still notify"
    );
    let (_fresh_dir, fresh_store) = fixture();
    // Compare schema through an independent fresh database, not version alone.
    let fresh = Connection::open(_fresh_dir.path().join("db")).unwrap();
    assert_eq!(schema(&c), schema(&fresh));
    drop(fresh_store);
}

#[test]
fn review_request_rechecks_owner_after_preflight_before_transactional_write() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct TransferDuringScopeCheck {
        other: MergeRequestStore,
        transfer: SetMergeRequestWorker,
        changed: AtomicBool,
    }
    impl RepositorySource for TransferDuringScopeCheck {
        fn repository_belongs_to_workspace(&self, _: &str, _: &str) -> Result<bool, String> {
            if !self.changed.swap(true, Ordering::SeqCst) {
                self.other
                    .set_worker(self.transfer.clone(), |_| Ok(()))
                    .map_err(|e| e.to_string())?;
            }
            Ok(true)
        }
    }
    let (dir, original) = fixture();
    open(&original);
    child(&original, "coder", "racing");
    let request = review_request(mr_auth(&original, "MR", "R"), "racing");
    let transfer = transfer(&original, "during-preflight", Some("b"));
    let repositories = TransferDuringScopeCheck {
        other: original,
        transfer,
        changed: AtomicBool::new(false),
    };
    let assignments = Assignments(Arc::new(Mutex::new(CurrentAssignment {
        assignment_id: "unused".into(),
        ticket_id: "T".into(),
        runtime_id: "runtime".into(),
        worker_id: "unused".into(),
    })));
    let store = MergeRequestStore::open(
        dir.path().join("db"),
        Arc::new(assignments),
        Arc::new(repositories),
    )
    .unwrap();
    assert!(matches!(
        store.request_review(request),
        Err(MergeRequestError::Unauthorized(_))
    ));
    let mr = store.get_by_id("W", "MR").unwrap();
    assert_eq!(mr.current_worker().unwrap().worker, Some(worker("b")));
    assert!(
        !mr.thread
            .iter()
            .any(|e| matches!(e, MergeRequestThreadEvent::ReviewRequested(_)))
    );
    assert!(store.authorize_review_submission("MR", "racing").is_err());
}
