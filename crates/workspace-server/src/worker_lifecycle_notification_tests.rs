// Real SQLite common-ledger tests. Runtime terminal production and delivery to a
// Controller are separate integration boundaries, not simulated by these tests.
fn lifecycle_grant(store: &SqliteWorkspaceStore, id: &str) -> WorkerControlGrantRecord {
    store
        .create_worker_control_grant(&WorkerControlGrantRecord {
            workspace_id: "w".into(),
            grant_id: id.into(),
            controller: RuntimeWorkerRef::new("r", "worker"),
            subject: RuntimeWorkerRef::new("r", "replacement"),
            relation: "spawned".into(),
            origin: "worker_spawn".into(),
            permissions: vec!["observe".into(), "notify".into()],
            operation_id: format!("spawn-{id}"),
            created_at: "2026-11-17T12:00:01Z".into(),
            revoked_at: None,
        })
        .unwrap()
}
fn lifecycle_event(id: &str, outcome: WorkerLifecycleOutcome) -> WorkerLifecycleEvent {
    WorkerLifecycleEvent {
        event_id: id.into(),
        occurred_at: "2026-11-17T12:00:00Z".into(),
        outcome,
        session_entry_ref: Some(format!("E{id}")),
        run_id: None,
    }
}
fn lifecycle_target() -> TicketWorkerNotificationTarget {
    TicketWorkerNotificationTarget {
        worker: RuntimeWorkerRef::new("r", "worker"),
        authentication_binding_id: "tracking-binding".into(),
    }
}

#[tokio::test]
async fn worker_lifecycle_source_replay_preserves_separate_runs_and_stop_after_reopen() {
    let (dir, store) = tracking_store_fixture().await;
    let grant = lifecycle_grant(&store, "spawn");
    let completed = lifecycle_event("run-1", WorkerLifecycleOutcome::Completed);
    // The child may have ended before persist_spawn_control committed.
    store
        .capture_worker_lifecycle("w", &grant.grant_id, &completed)
        .unwrap();
    store
        .capture_worker_lifecycle("w", &grant.grant_id, &completed)
        .unwrap();
    let mut changed = completed.clone();
    changed.outcome = WorkerLifecycleOutcome::Errored;
    assert!(
        store
            .capture_worker_lifecycle("w", &grant.grant_id, &changed)
            .is_err()
    );
    for event in [
        lifecycle_event("run-2", WorkerLifecycleOutcome::Errored),
        lifecycle_event("stop-1", WorkerLifecycleOutcome::Stopped),
    ] {
        store
            .capture_worker_lifecycle("w", &grant.grant_id, &event)
            .unwrap();
    }
    drop(store);
    let reopened = SqliteWorkspaceStore::open_for_test(dir.path().join("workspace.db")).unwrap();
    let notices = reopened.pending_worker_lifecycle("w", 10).unwrap();
    assert_eq!(notices.len(), 3);
    assert!(
        reopened
            .pending_worker_lifecycle("other", 10)
            .unwrap()
            .is_empty()
    );
    let notice = &notices[0];
    let target = lifecycle_target();
    assert_eq!(
        reopened
            .prepare_worker_lifecycle("w", notice, "fixed bytes", &target)
            .unwrap(),
        "fixed bytes"
    );
    reopened
        .record_notification_delivery_failure(
            "w",
            &NotificationDeliveryKey::WorkerNotification {
                delivery_id: notice.delivery_id.clone(),
            },
            NotificationFailureCode::AcceptanceUnknown,
        )
        .unwrap();
    assert_eq!(
        reopened
            .prepare_worker_lifecycle("w", notice, "rerendered", &target)
            .unwrap(),
        "fixed bytes"
    );
    let diagnostics = reopened
        .notification_diagnostics(
            "w",
            &server_api::NotificationDiagnosticsQuery {
                source_event_id: Some(notice.delivery_id.clone()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(diagnostics.items.len(), 1);
    assert!(diagnostics.items[0].acceptance_outcome_unknown);
    assert!(!diagnostics.items[0].model_processing_confirmed);
    let replacement = TicketWorkerNotificationTarget {
        worker: RuntimeWorkerRef::new("r", "replacement"),
        ..target.clone()
    };
    assert!(
        reopened
            .prepare_worker_lifecycle("w", notice, "fixed bytes", &replacement)
            .is_err()
    );
    assert!(
        reopened
            .ack_worker_lifecycle("w", &notice.delivery_id, &replacement)
            .is_err()
    );
    reopened
        .ack_worker_lifecycle("w", &notice.delivery_id, &target)
        .unwrap();
    reopened
        .ack_worker_lifecycle("w", &notice.delivery_id, &target)
        .unwrap();
    assert_eq!(reopened.pending_worker_lifecycle("w", 10).unwrap().len(), 2);
    let diagnostics = reopened
        .notification_diagnostics(
            "w",
            &server_api::NotificationDiagnosticsQuery {
                source_event_id: Some(notice.delivery_id.clone()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        diagnostics.items[0].stage,
        server_api::NotificationDiagnosticStage::RuntimeAccepted
    );
}

#[tokio::test]
async fn worker_lifecycle_revoked_relation_cannot_prepare_or_starve_other_sources() {
    let (_dir, store) = tracking_store_fixture().await;
    let grant = lifecycle_grant(&store, "spawn");
    for id in ["run-1", "run-2"] {
        store
            .capture_worker_lifecycle(
                "w",
                &grant.grant_id,
                &lifecycle_event(id, WorkerLifecycleOutcome::Cancelled),
            )
            .unwrap();
    }
    let one = store.pending_worker_lifecycle("w", 1).unwrap().remove(0);
    let two = store.pending_worker_lifecycle("w", 1).unwrap().remove(0);
    assert_ne!(one.delivery_id, two.delivery_id);
    store
        .prepare_worker_lifecycle("w", &one, "saved", &lifecycle_target())
        .unwrap();
    store
        .revoke_worker_control_grant("w", &grant.grant_id, "2026-11-17T13:00:00Z")
        .unwrap();
    assert!(
        store
            .prepare_worker_lifecycle("w", &one, "saved", &lifecycle_target())
            .is_err()
    );
    assert!(
        store
            .prepare_worker_lifecycle("w", &two, "new", &lifecycle_target())
            .is_err()
    );
    store
        .capture_worker_lifecycle(
            "w",
            &grant.grant_id,
            &lifecycle_event("late", WorkerLifecycleOutcome::Stopped),
        )
        .unwrap();
    assert_eq!(store.pending_worker_lifecycle("w", 10).unwrap().len(), 2);
    assert!(store.lifecycle_scan_targets("w", 10).unwrap().is_empty());
    // Ticket recovery must neither decode nor reinterpret lifecycle sources.
    store
        .with_conn(|conn| notification_routing::route_pending(conn, "w"))
        .unwrap();
}

#[tokio::test]
async fn worker_lifecycle_concurrent_capture_and_failed_routing_replay_one_source() {
    let (_dir, store) = tracking_store_fixture().await;
    let grant = lifecycle_grant(&store, "spawn");
    let fact = lifecycle_event("run-1", WorkerLifecycleOutcome::Limited);
    store.with_conn(|conn| {
        conn.execute_batch("CREATE TRIGGER lifecycle_fail_decision BEFORE INSERT ON notification_decisions BEGIN SELECT RAISE(ABORT,'injected routing save failure'); END;")?;
        Ok(())
    }).unwrap();
    assert!(store.capture_worker_lifecycle("w", &grant.grant_id, &fact).is_err());
    store.with_conn(|conn| {
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM notification_sources WHERE role='worker_lifecycle'", [], |row| row.get(0))?;
        assert_eq!(count, 0, "source and routing decision must roll back together");
        conn.execute_batch("DROP TRIGGER lifecycle_fail_decision")?;
        Ok(())
    }).unwrap();
    std::thread::scope(|scope| {
        let threads: Vec<_> = (0..4).map(|_| scope.spawn(|| {
            store.capture_worker_lifecycle("w", &grant.grant_id, &fact).unwrap();
        })).collect();
        for thread in threads { thread.join().unwrap(); }
    });
    let notices = store.pending_worker_lifecycle("w", 20).unwrap();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].event.outcome, WorkerLifecycleOutcome::Limited);
    let old = store.lifecycle_scan_targets("w", 1).unwrap().remove(0);
    store.checkpoint_worker_lifecycle("w", &old, Some("{}"), Some("stop-1")).unwrap();
    assert!(store.checkpoint_worker_lifecycle("w", &old, None, None).is_err());
    let current = store.lifecycle_scan_targets("w", 1).unwrap().remove(0);
    assert_eq!(current.run_cursor.as_deref(), Some("{}"));
    assert_eq!(current.stop_after.as_deref(), Some("stop-1"));
}

#[tokio::test]
async fn worker_lifecycle_scan_is_bounded_fair_and_requires_spawn_relation() {
    let (_dir, store) = tracking_store_fixture().await;
    lifecycle_grant(&store, "first");
    lifecycle_grant(&store, "second");
    let first = store.lifecycle_scan_targets("w", 1).unwrap().remove(0);
    let second = store.lifecycle_scan_targets("w", 1).unwrap().remove(0);
    assert_ne!(first.grant.grant_id, second.grant.grant_id);
    assert!(
        store
            .capture_worker_lifecycle(
                "w",
                "missing",
                &lifecycle_event("e", WorkerLifecycleOutcome::Completed)
            )
            .is_err()
    );
    assert!(
        store
            .capture_worker_lifecycle(
                "other",
                &first.grant.grant_id,
                &lifecycle_event("e", WorkerLifecycleOutcome::Completed)
            )
            .is_err()
    );
}
