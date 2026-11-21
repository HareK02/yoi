//! Real SQLite query coverage; no model or Git/provider effects are simulated as evidence.
use super::*;

fn pool() -> MergeRequestListQuery {
    MergeRequestListQuery {
        phase: Some(MergeRequestPhase::Integration),
        repository_id: Some("R".into()),
        selector_to: Some("refs/heads/develop".into()),
        limit: 10,
        ..Default::default()
    }
}

#[test]
fn query_pool_is_phase_based_not_approval_based_and_normalizes_target() {
    let (dir, store) = fixture();
    open(&store);
    open_for(&store, "MR-2", "R2");
    assert!(store.list("W", &pool()).unwrap().items.is_empty());
    approve(&store, "one", "approved");
    approve_for(&store, "MR-2", "R2", "other", "other-approved");
    assert_eq!(store.list("W", &pool()).unwrap().items.len(), 1);
    request(&store, "changed-source", "changes");
    store
        .submit_review(SubmitMergeRequestReview {
            merge_request_id: "MR".into(),
            ticket_id: "T".into(),
            current_subject_ref: "changed-source".into(),
            capability_token: "changes".into(),
            decision: ReviewDecision::RequestChanges,
            body: "conflict requires another fix".into(),
            findings: vec![],
            now: at(5),
        })
        .unwrap();
    let page = store.list("W", &pool()).unwrap();
    assert_eq!(page.items[0].merge_request_id, "MR");
    assert_eq!(page.items[0].phase(), MergeRequestPhase::Integration);
    assert!(
        store
            .list(
                "W",
                &MergeRequestListQuery {
                    selector_to: Some("release".into()),
                    ..pool()
                }
            )
            .unwrap()
            .items
            .is_empty()
    );
    let connection = Connection::open(dir.path().join("db")).unwrap();
    connection.execute("UPDATE merge_requests SET selector_to='refs/heads/develop' WHERE merge_request_id='MR'", []).unwrap();
    assert_eq!(
        store
            .list(
                "W",
                &MergeRequestListQuery {
                    selector_to: Some("develop".into()),
                    ..pool()
                }
            )
            .unwrap()
            .items
            .len(),
        1
    );
    connection.execute("UPDATE merge_requests SET state=json_object('kind','canceled','from',state -> '$') WHERE merge_request_id='MR'", []).unwrap();
    assert!(store.list("W", &pool()).unwrap().items.is_empty());
    assert_eq!(
        store
            .list(
                "W",
                &MergeRequestListQuery {
                    phase: Some(MergeRequestPhase::Canceled),
                    ..pool()
                }
            )
            .unwrap()
            .items
            .len(),
        1
    );
}

#[test]
fn query_assignment_uses_latest_worker_event_not_history_or_ticket_owner() {
    let (_dir, store) = fixture();
    open(&store);
    let worker = WorkerIdentity {
        runtime_id: "runtime".into(),
        worker_id: "coder".into(),
    };
    let query = MergeRequestListQuery {
        assigned_worker: Some(worker.clone()),
        limit: 10,
        ..Default::default()
    };
    assert_eq!(store.list("W", &query).unwrap().items.len(), 1);
    let original = store.current_worker("W", "MR").unwrap().unwrap();
    let reassigned = store
        .set_worker(
            SetMergeRequestWorker {
                workspace_id: "W".into(),
                merge_request_id: "MR".into(),
                operation_id: "handoff".into(),
                expected_operation_id: Some(original.operation_id),
                worker: Some(WorkerIdentity {
                    runtime_id: "other-runtime".into(),
                    worker_id: "coder".into(),
                }),
                assigned_by: worker.clone(),
                now: at(5),
            },
            |_| Ok(()),
        )
        .unwrap();
    assert!(store.list("W", &query).unwrap().items.is_empty());
    let other = MergeRequestListQuery {
        assigned_worker: reassigned.worker.clone(),
        ..query.clone()
    };
    assert_eq!(store.list("W", &other).unwrap().items.len(), 1);
    store
        .set_worker(
            SetMergeRequestWorker {
                workspace_id: "W".into(),
                merge_request_id: "MR".into(),
                operation_id: "release".into(),
                expected_operation_id: Some(reassigned.operation_id),
                worker: None,
                assigned_by: worker,
                now: at(6),
            },
            |_| Ok(()),
        )
        .unwrap();
    assert!(store.list("W", &other).unwrap().items.is_empty());
}

#[test]
fn query_cursor_freezes_position_and_rejects_workspace_or_filter_changes() {
    let (dir, store) = fixture();
    open(&store);
    open_for(&store, "MR-2", "R2");
    let mut query = MergeRequestListQuery {
        limit: 1,
        ..Default::default()
    };
    let page = store.list("W", &query).unwrap();
    assert_eq!(page.items[0].merge_request_id, "MR");
    query.cursor = page.next_cursor;
    // If the cursor were resolved from the mutable MR row, this update would skip MR-2.
    Connection::open(dir.path().join("db"))
        .unwrap()
        .execute(
            "UPDATE merge_requests SET updated_at='2000' WHERE merge_request_id='MR'",
            [],
        )
        .unwrap();
    let next = store.list("W", &query).unwrap();
    assert_eq!(next.items[0].merge_request_id, "MR-2");
    assert!(store.list("other", &query).is_err());
    assert!(
        store
            .list(
                "W",
                &MergeRequestListQuery {
                    phase: Some(MergeRequestPhase::Integration),
                    ..query
                }
            )
            .is_err()
    );
}
