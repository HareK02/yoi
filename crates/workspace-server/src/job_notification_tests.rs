// Real SQLite result transactions, migration, reopen and concurrent recovery.
fn job_notice_fixture(
    store: &SqliteWorkspaceStore,
    id: &str,
    target: bool,
) -> (BackendJobResultAcceptance, JobNotificationSource) {
    let mut request = resource_job_request(id, id, id);
    request.purpose = "notification_test".into();
    request.grants = Default::default();
    request.profile = "builtin:backend-job".into();
    request.notification_target = target.then(|| RuntimeWorkerRef::new("r", "replacement"));
    let worker = RuntimeWorkerRef::new("r", format!("job-worker-{id}"));
    let mut registration = store
        .get_worker_registry("w", &RuntimeWorkerRef::new("r", "replacement"))
        .unwrap()
        .unwrap();
    registration.worker = worker.clone();
    store.upsert_worker_registry(&registration).unwrap();
    let reservation = store
        .reserve_backend_job("w", &request, RESOURCE_JOB_NOW)
        .unwrap();
    store
        .claim_backend_job_attempt_dispatch(
            "w",
            id,
            &reservation.attempt.attempt_id,
            RESOURCE_JOB_NOW,
        )
        .unwrap();
    store
        .bind_backend_job_attempt_worker(
            "w",
            id,
            &reservation.attempt.attempt_id,
            &worker,
            Some("run"),
            RESOURCE_JOB_NOW,
        )
        .unwrap();
    let acceptance = store
        .accept_backend_job_result(
            "w",
            &worker,
            &BackendJobResultSubmission {
                job_id: id.into(),
                attempt_id: reservation.attempt.attempt_id.clone(),
                input_digest: request.input_digest().unwrap(),
                result: serde_json::json!({"valid":true}),
            },
            RESOURCE_JOB_NOW,
        )
        .unwrap();
    let source = JobNotificationSource {
        job_id: id.into(),
        attempt_id: acceptance.attempt.attempt_id.clone(),
        delivery_id: crate::backend_job::delivery_id(id, &acceptance.attempt.attempt_id),
    };
    (acceptance, source)
}

fn job_notice_evidence(
    store: &SqliteWorkspaceStore,
    source: &JobNotificationSource,
) -> (String, String, bool, Option<String>) {
    store.with_conn(|conn|Ok(conn.query_row("SELECT disposition,reason,EXISTS(SELECT 1 FROM notification_acknowledgements WHERE workspace_id='w' AND delivery_id=?1),(SELECT code FROM notification_failures WHERE workspace_id='w' AND delivery_id=?1 ORDER BY sequence DESC LIMIT 1) FROM notification_decisions WHERE workspace_id='w' AND delivery_id=?1",[&source.delivery_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?)).unwrap()
}

#[tokio::test]
async fn job_notification_routing_common_policy_distinguishes_suppression_failures_and_acceptance()
{
    let (_directory, store) = tracking_store_fixture().await;
    for (name, target, facts, disposition, reason, failure) in [
        (
            "normal",
            true,
            JobNotificationFacts::Result("result notice".into()),
            "deliver",
            "result",
            None,
        ),
        (
            "no-target",
            false,
            JobNotificationFacts::Result("unused".into()),
            "suppress",
            "no_recipient",
            None,
        ),
        (
            "clean",
            true,
            JobNotificationFacts::NoFindings,
            "suppress",
            "no_findings",
            None,
        ),
        (
            "stale",
            true,
            JobNotificationFacts::StaleContent,
            "suppress",
            "stale_content",
            None,
        ),
        (
            "invalid",
            true,
            JobNotificationFacts::Failed(NotificationFailureCode::InvalidResult),
            "defer",
            "source_unavailable",
            Some("invalid_result"),
        ),
        (
            "oversize",
            true,
            JobNotificationFacts::Result(
                "x".repeat(crate::backend_job::MAX_JOB_DELIVERY_BYTES + 1),
            ),
            "defer",
            "source_unavailable",
            Some("content_too_large"),
        ),
        (
            "render",
            true,
            JobNotificationFacts::Result(String::new()),
            "defer",
            "source_unavailable",
            Some("rendering_failed"),
        ),
    ] {
        let (acceptance, source) = job_notice_fixture(&store, name, target);
        assert_eq!(
            store.route_job_notification("w", &source, &facts).unwrap(),
            disposition == "deliver"
        );
        assert_eq!(
            job_notice_evidence(&store, &source),
            (
                disposition.into(),
                reason.into(),
                false,
                failure.map(String::from)
            )
        );
        assert_eq!(
            store.get_backend_job("w", name).unwrap().unwrap().result,
            acceptance.job.result
        );
        assert_eq!(
            store.get_backend_job("w", name).unwrap().unwrap().state,
            BackendJobState::Completed
        );
        if disposition != "deliver" {
            assert!(
                store
                    .job_notification_request("w", &source.delivery_id)
                    .unwrap()
                    .is_none()
            );
            assert!(
                !store
                    .route_job_notification(
                        "w",
                        &source,
                        &JobNotificationFacts::Result("new rendering".into())
                    )
                    .unwrap()
            );
        }
    }
    assert_eq!(
        store
            .list_pending_job_notifications("w", 100)
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn job_notification_preparation_failures_remain_pending_and_append_only() {
    let (_directory, store) = tracking_store_fixture().await;
    for target in [true, false] {
        let (_, source) = job_notice_fixture(&store, if target { "retry" } else { "retry-no-target" }, target);
        for _ in 0..2 {
            assert!(!store.route_job_notification("w", &source, &JobNotificationFacts::Failed(NotificationFailureCode::SourceLookupFailed)).unwrap());
        }
        store.with_conn(|conn| {
            let sequence: i64 = conn.query_row("SELECT sequence FROM notification_sources WHERE input_event_id=?1", [&source.delivery_id], |r| r.get(0))?;
            assert_eq!(conn.query_row("SELECT COUNT(*) FROM notification_source_preparation_failures WHERE source_sequence=?1 AND code='source_lookup_failed'", [sequence], |r| r.get::<_, i64>(0))?, 2);
            assert_eq!(conn.query_row("SELECT COUNT(*) FROM notification_decisions WHERE source_sequence=?1", [sequence], |r| r.get::<_, i64>(0))?, 0);
            assert!(conn.execute("UPDATE notification_source_preparation_failures SET code='invalid_result' WHERE source_sequence=?1", [sequence]).is_err());
            assert!(conn.execute("DELETE FROM notification_source_preparation_failures WHERE source_sequence=?1", [sequence]).is_err());
            assert!(conn.execute("INSERT INTO notification_source_preparation_failures(source_sequence,code) VALUES(-1,'source_lookup_failed')", []).is_err());
            Ok(())
        }).unwrap();
        assert!(store.list_pending_job_notifications("w", 100).unwrap().iter().any(|s| s.delivery_id == source.delivery_id));
        assert_eq!(store.route_job_notification("w", &source, &JobNotificationFacts::Result("recovered body".into())).unwrap(), target);
        let pinned = store.job_notification_request("w", &source.delivery_id).unwrap();
        // Later failed reads cannot overwrite the pinned decision/body/recipient
        // or fabricate a new pre-decision observation after routing has finished.
        assert_eq!(store.route_job_notification("w", &source, &JobNotificationFacts::Failed(NotificationFailureCode::SourceLookupFailed)).unwrap(), target);
        assert_eq!(store.job_notification_request("w", &source.delivery_id).unwrap(), pinned);
        store.with_conn(|conn| {
            assert_eq!(conn.query_row("SELECT COUNT(*) FROM notification_source_preparation_failures f JOIN notification_sources s ON s.sequence=f.source_sequence WHERE s.input_event_id=?1", [&source.delivery_id], |r| r.get::<_, i64>(0))?, 2);
            Ok(())
        }).unwrap();
    }
}

#[tokio::test]
async fn job_notification_fair_offers_survive_reopen_and_bound_preparation_and_ack_retries() {
    let (directory, mut store) = tracking_store_fixture().await;
    let mut failing = Vec::new();
    for i in 0..25 {
        let (_, source) = job_notice_fixture(&store, &format!("failing-{i}"), true);
        assert!(!store.route_job_notification(
            "w", &source,
            &JobNotificationFacts::Failed(NotificationFailureCode::SourceLookupFailed),
        ).unwrap());
        failing.push(source);
    }
    let (_, healthy) = job_notice_fixture(&store, "later-healthy", true);
    let (_, retry) = job_notice_fixture(&store, "later-pinned-retry", true);
    let target = TicketWorkerNotificationTarget {
        worker: RuntimeWorkerRef::new("r", "replacement"),
        authentication_binding_id: "tracking-binding".into(),
    };
    assert!(store.route_job_notification(
        "w", &retry, &JobNotificationFacts::Result("original pinned bytes".into()),
    ).unwrap());
    assert_eq!(store.authorize_job_notification("w", &retry.delivery_id, &target).unwrap(), None);
    let retry_key = NotificationDeliveryKey::WorkerNotification {
        delivery_id: retry.delivery_id.clone(),
    };
    store.record_notification_delivery_failure(
        "w", &retry_key, NotificationFailureCode::AcceptanceUnknown,
    ).unwrap();
    let pinned = store.job_notification_request("w", &retry.delivery_id).unwrap();
    // Start with an already-offered pinned retry, not only a never-offered result.
    // All sources share the same offer clock; 25 failure observations are not ticks.
    assert_eq!(store.list_pending_job_notifications("w", 100).unwrap().len(), 27);
    let snapshot = |store: &SqliteWorkspaceStore, table: &str| {
        store.with_conn(|conn| Ok(stored_fixture_rows(conn, table))).unwrap()
    };
    let original_sources = snapshot(&store, "notification_sources");
    let original_decisions = snapshot(&store, "notification_decisions");
    let original_preparation_failures = snapshot(&store, "notification_source_preparation_failures");
    let original_delivery_failures = snapshot(&store, "notification_failures");
    let original_jobs = snapshot(&store, "backend_jobs");
    let mut failure_offers = vec![0; failing.len()];
    let mut healthy_round = None;
    let mut retry_rounds = Vec::new();

    for round in 1..=4 {
        // Reopen before every production-sized round, including the first retry.
        let saved_offers = snapshot(&store, "notification_source_offers");
        drop(store);
        store = SqliteWorkspaceStore::open_for_test(directory.path().join("workspace.db")).unwrap();
        assert_eq!(snapshot(&store, "notification_source_offers"), saved_offers);
        let pending = store.list_pending_job_notifications("w", 20).unwrap();
        assert_eq!(pending.len(), 20);
        if round == 1 {
            assert!(pending.iter().all(|s| s.job_id.starts_with("failing-")));
        }
        for source in pending {
            if let Some(index) = failing.iter().position(|s| s.delivery_id == source.delivery_id) {
                failure_offers[index] += 1;
                assert!(!store.route_job_notification(
                    "w", &source,
                    &JobNotificationFacts::Failed(NotificationFailureCode::SourceLookupFailed),
                ).unwrap());
            } else if source.delivery_id == healthy.delivery_id {
                assert!(healthy_round.replace(round).is_none());
                assert!(store.route_job_notification(
                    "w", &source, &JobNotificationFacts::Result("healthy body".into()),
                ).unwrap());
                assert_eq!(store.authorize_job_notification("w", &source.delivery_id, &target).unwrap(), None);
                store.ack_job_notification("w", &source.delivery_id, &target.worker).unwrap();
            } else {
                assert_eq!(source.delivery_id, retry.delivery_id);
                retry_rounds.push(round);
                // Neither scheduling nor a new rendering may replace the request.
                assert!(store.route_job_notification(
                    "w", &source, &JobNotificationFacts::Result("must not replace pinned bytes".into()),
                ).unwrap());
                assert_eq!(store.job_notification_request("w", &source.delivery_id).unwrap(), pinned);
                if retry_rounds.len() == 1 {
                    store.record_notification_delivery_failure(
                        "w", &retry_key, NotificationFailureCode::AcceptanceUnknown,
                    ).unwrap();
                } else {
                    store.ack_job_notification("w", &source.delivery_id, &target.worker).unwrap();
                }
            }
        }
        if round == 2 {
            assert!(failure_offers.iter().all(|count| *count >= 1));
            assert_eq!(healthy_round, Some(2));
            assert_eq!(retry_rounds, vec![2]);
        }
    }
    // With 26 sources still pending after the healthy ACK, even the last source
    // in an offer-time tie returns within ceil(26 / 20) = 2 further rounds.
    assert_eq!(retry_rounds, vec![2, 4], "a pinned retry must not starve behind undecided sources");
    assert!(job_notice_evidence(&store, &healthy).2);
    assert!(job_notice_evidence(&store, &retry).2);
    assert_eq!(snapshot(&store, "notification_sources"), original_sources);
    assert_eq!(snapshot(&store, "backend_jobs"), original_jobs);
    let decisions = snapshot(&store, "notification_decisions");
    assert!(original_decisions.iter().all(|row| decisions.contains(row)));
    let preparation_failures = snapshot(&store, "notification_source_preparation_failures");
    assert!(preparation_failures.starts_with(&original_preparation_failures));
    assert_eq!(preparation_failures.len(), 25 + failure_offers.iter().sum::<usize>());
    assert!(snapshot(&store, "notification_failures").starts_with(&original_delivery_failures));
    assert_eq!(store.job_notification_request("w", &retry.delivery_id).unwrap(), pinned);
    assert!(store.list_pending_job_notifications("w", 20).unwrap().iter().all(|s| s.job_id.starts_with("failing-")));
    store.with_conn(|conn| {
        assert!(conn.execute("INSERT INTO notification_source_offers VALUES(-1,1)", []).is_err());
        assert!(conn.execute("UPDATE notification_source_offers SET last_offered=0", []).is_err());
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM notification_sources s JOIN notification_decisions d ON d.source_sequence=s.sequence WHERE s.role='backend_job'", [], |r| r.get::<_, i64>(0))?, 2);
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r.get::<_, i64>(0))?, 0);
        Ok(())
    }).unwrap();
}

#[tokio::test]
async fn job_notification_result_commit_reopen_parallel_recovery_and_fixed_target_receipt() {
    let (directory, store) = tracking_store_fixture().await;
    let (accepted, source) = job_notice_fixture(&store, "commit-gap", true);
    // No delivery reservation or routing callback has run. The result transaction
    // alone leaves one recoverable source, including after reopening SQLite.
    assert!(
        store
            .job_notification_request("w", &source.delivery_id)
            .unwrap()
            .is_none()
    );
    let submission = BackendJobResultSubmission {
        job_id: source.job_id.clone(),
        attempt_id: source.attempt_id.clone(),
        input_digest: accepted.attempt.input_digest.clone(),
        result: accepted.job.result.clone().unwrap(),
    };
    assert!(
        store
            .accept_backend_job_result(
                "w",
                accepted.attempt.worker.as_ref().unwrap(),
                &submission,
                RESOURCE_JOB_NOW
            )
            .unwrap()
            .replayed
    );
    assert!(
        store
            .accept_backend_job_result(
                "other",
                accepted.attempt.worker.as_ref().unwrap(),
                &submission,
                RESOURCE_JOB_NOW
            )
            .is_err()
    );
    let mut stale = submission.clone();
    stale.attempt_id = "old-attempt".into();
    assert!(
        store
            .accept_backend_job_result(
                "w",
                accepted.attempt.worker.as_ref().unwrap(),
                &stale,
                RESOURCE_JOB_NOW
            )
            .is_err()
    );
    drop(store);
    let path = directory.path().join("workspace.db");
    let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    assert_eq!(
        store
            .list_pending_job_notifications("w", 100)
            .unwrap()
            .len(),
        1
    );
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads = (0..2)
        .map(|i| {
            let path = path.clone();
            let source = source.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let store = SqliteWorkspaceStore::open_for_test(path).unwrap();
                barrier.wait();
                store
                    .route_job_notification(
                        "w",
                        &source,
                        &JobNotificationFacts::Result(format!("immutable rendering {i}")),
                    )
                    .unwrap();
                store
                    .job_notification_request("w", &source.delivery_id)
                    .unwrap()
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    let results = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results[0], results[1]);
    assert!(
        store
            .route_job_notification("w", &source, &JobNotificationFacts::StaleContent)
            .unwrap()
    );
    assert_eq!(
        store
            .job_notification_request("w", &source.delivery_id)
            .unwrap()
            .unwrap(),
        results[0]
    );
    let target = TicketWorkerNotificationTarget {
        worker: RuntimeWorkerRef::new("r", "replacement"),
        authentication_binding_id: "tracking-binding".into(),
    };
    assert_eq!(
        store
            .authorize_job_notification("w", &source.delivery_id, &target)
            .unwrap(),
        None
    );
    let mut replacement = target.clone();
    replacement.worker.worker_id = "another-worker".into();
    assert!(
        store
            .authorize_job_notification("w", &source.delivery_id, &replacement)
            .unwrap()
            .is_some()
    );
    replacement = target.clone();
    replacement.authentication_binding_id = "replacement-binding".into();
    assert!(
        store
            .authorize_job_notification("w", &source.delivery_id, &replacement)
            .unwrap()
            .is_some()
    );
    store
        .record_notification_delivery_failure(
            "w",
            &NotificationDeliveryKey::WorkerNotification {
                delivery_id: source.delivery_id.clone(),
            },
            NotificationFailureCode::AcceptanceUnknown,
        )
        .unwrap();
    assert!(!job_notice_evidence(&store, &source).2);
    assert_eq!(
        store
            .job_notification_request("w", &source.delivery_id)
            .unwrap()
            .unwrap(),
        results[0]
    );
    store.with_conn(|conn| {conn.execute("UPDATE workspace_runtime_bindings SET revoked_at=?1 WHERE workspace_id='w' AND runtime_id='r'",[RESOURCE_JOB_NOW])?;Ok(())}).unwrap();
    assert_eq!(
        store
            .authorize_job_notification("w", &source.delivery_id, &target)
            .unwrap(),
        Some(NotificationFailureCode::BindingRevoked)
    );
    // A receipt about the already authorized send remains valid after revocation.
    store
        .ack_job_notification("w", &source.delivery_id, &target.worker)
        .unwrap();
    store
        .ack_job_notification("w", &source.delivery_id, &target.worker)
        .unwrap();
    assert!(job_notice_evidence(&store, &source).2);
    assert!(
        store
            .list_pending_job_notifications("w", 100)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .get_backend_job("w", &source.job_id)
            .unwrap()
            .unwrap()
            .current_attempt,
        1
    );
    store
        .with_conn(|conn| {
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM notification_sources WHERE role='backend_job'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                1
            );
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM notification_requests", [], |r| r
                    .get::<_, i64>(0))?,
                1
            );
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r
                    .get::<_, i64>(
                    0
                ))?,
                0
            );
            Ok(())
        })
        .unwrap();
}

fn job_notice_legacy_seed(
    conn: &Connection,
    name: &str,
    state: Option<&str>,
    category: Option<&str>,
    target: bool,
) -> String {
    let mut request = resource_job_request(name, name, name);
    request.purpose = "legacy_notice".into();
    request.grants = Default::default();
    request.notification_target =
        target.then(|| RuntimeWorkerRef::new("old-runtime", "old-worker"));
    let attempt = attempt_id(name, 1);
    let id = crate::backend_job::delivery_id(name, &attempt);
    conn.execute("INSERT INTO backend_jobs(workspace_id,job_id,purpose,input_digest,input_ref,request_json,intent_fingerprint,state,current_attempt,result_json,result_digest,created_at,updated_at,completed_at) VALUES('w',?1,'legacy_notice',?2,'test://input',?3,'fingerprint','completed',1,'{}','digest',?4,?4,?4)",params![name,request.input_digest().unwrap(),serde_json::to_string(&request).unwrap(),RESOURCE_JOB_NOW]).unwrap();
    conn.execute("INSERT INTO backend_job_attempts(workspace_id,job_id,attempt_id,attempt,input_digest,state,deadline_at,result_json,result_digest,created_at,updated_at,completed_at) VALUES('w',?1,?2,1,?3,'completed',?4,'{}','digest',?4,?4,?4)",params![name,attempt,request.input_digest().unwrap(),RESOURCE_JOB_NOW]).unwrap();
    if let Some(state) = state {
        conn.execute("INSERT INTO backend_job_deliveries(workspace_id,delivery_id,job_id,attempt_id,target_runtime_id,target_worker_id,state,failure_category,created_at,updated_at,delivered_at) VALUES('w',?1,?2,?3,'old-runtime','old-worker',?4,?5,?6,?6,CASE WHEN ?4='completed' THEN ?6 ELSE NULL END)",params![id,name,attempt,state,category,RESOURCE_JOB_NOW]).unwrap();
    }
    id
}

#[test]
fn job_notification_migration_retains_legacy_acks_suppressions_unknown_bytes_and_commit_gaps() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("workspace.db");
    let conn = tracking_schema91_fixture(&path);
    migrate_ticket_tracking_v91_to_v92(&conn).unwrap();
    migrate_assignment_delivery_v92_to_v93(&conn).unwrap();
    migrate_ticket_posts_v93_to_v94(&conn).unwrap();
    migrate_notification_routing_v94_to_v95(&conn).unwrap();
    // Populate the v95 common graph before rebuilding its source parent. Include
    // both accepted and retryable Ticket requests, with retained failure history.
    for id in ["ticket-ack", "ticket-pending"] {
        conn.execute("INSERT INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,facts_json) VALUES('w','pending',?1,'worker','ticket-worker-assignment','comment','{\"principal_kind\":\"worker\",\"principal_id\":null,\"runtime_id\":\"r\",\"worker_id\":\"worker\",\"status\":null,\"case_state\":\"open\",\"released\":false}')", [id]).unwrap();
        conn.execute("INSERT INTO notification_decisions(source_sequence,workspace_id,delivery_id,disposition,reason,purpose,last_offered) SELECT sequence,'w',?1,'deliver','attention','attention',17 FROM notification_sources WHERE input_event_id=?1", [id]).unwrap();
        conn.execute("INSERT INTO notification_requests VALUES('w',?1,'original Ticket bytes','{\"worker\":{\"runtime_id\":\"r\",\"worker_id\":\"worker\"},\"authentication_binding_id\":\"original-binding\"}')", [id]).unwrap();
        conn.execute("INSERT INTO notification_failures(workspace_id,delivery_id,code,recorded_at) VALUES('w',?1,'acceptance_unknown','original-failure-time')", [id]).unwrap();
    }
    conn.execute("INSERT INTO notification_acknowledgements SELECT workspace_id,delivery_id,target_json FROM notification_requests WHERE delivery_id='ticket-ack'", []).unwrap();
    let ticket_graph = |conn: &Connection| {
        [
            "SELECT * FROM notification_sources WHERE role!='backend_job' ORDER BY sequence".to_string(),
            "SELECT d.* FROM notification_decisions d JOIN notification_sources s ON s.sequence=d.source_sequence WHERE s.role!='backend_job' ORDER BY d.rowid".to_string(),
        ].into_iter().chain(["notification_requests", "notification_acknowledgements", "notification_failures"].map(|table| format!("SELECT f.* FROM {table} f JOIN notification_decisions d USING(workspace_id,delivery_id) JOIN notification_sources s ON s.sequence=d.source_sequence WHERE s.role!='backend_job' ORDER BY f.rowid")))
            .map(|sql| fixture_query_rows(conn, &sql)).collect::<Vec<_>>()
    };
    let ticket_before = ticket_graph(&conn);
    assert!(ticket_before.iter().all(|rows| !rows.is_empty()));
    let schema_before = fixture_query_rows(&conn, "SELECT type,name,sql FROM sqlite_schema WHERE type IN ('trigger','index') AND sql IS NOT NULL ORDER BY name");
    let check_guards = |conn: &Connection| {
        for sql in [
            "UPDATE notification_sources SET kind='changed' WHERE input_event_id='ticket-ack'",
            "DELETE FROM notification_sources WHERE input_event_id='ticket-ack'",
            "UPDATE notification_decisions SET reason='changed' WHERE delivery_id='ticket-ack'",
            "DELETE FROM notification_decisions WHERE delivery_id='ticket-ack'",
            "UPDATE notification_requests SET content='changed' WHERE delivery_id='ticket-ack'",
            "DELETE FROM notification_acknowledgements WHERE delivery_id='ticket-ack'",
            "UPDATE notification_failures SET code='changed' WHERE delivery_id='ticket-ack'",
            "INSERT INTO notification_decisions(source_sequence,workspace_id,delivery_id,disposition,reason,purpose) VALUES(-1,'w','orphan','deliver','result','result')",
            "INSERT INTO notification_requests VALUES('w','orphan','content','{}')",
            "INSERT INTO notification_acknowledgements VALUES('w','orphan','{}')",
            "INSERT INTO notification_failures(workspace_id,delivery_id,code) VALUES('w','orphan','acceptance_unknown')",
        ] {
            assert!(conn.execute_batch(sql).is_err(), "guard lost: {sql}");
        }
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        assert_eq!(conn.query_row("PRAGMA legacy_alter_table", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        assert_eq!(conn.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
    };
    check_guards(&conn);
    let accepted = job_notice_legacy_seed(&conn, "ack", Some("completed"), None, true);
    let clean = job_notice_legacy_seed(
        &conn,
        "clean",
        Some("completed"),
        Some("notification_suppressed_no_findings"),
        true,
    );
    let stale = job_notice_legacy_seed(
        &conn,
        "stale",
        Some("completed"),
        Some("notification_suppressed_stale_ticket_content"),
        true,
    );
    let unknown = job_notice_legacy_seed(
        &conn,
        "unknown",
        Some("unknown"),
        Some("notification_outcome_unknown"),
        true,
    );
    let ambiguous = job_notice_legacy_seed(
        &conn,
        "ambiguous",
        Some("completed"),
        Some("unrecognized_legacy_reason"),
        true,
    );
    job_notice_legacy_seed(&conn, "no-target", None, None, false);
    job_notice_legacy_seed(&conn, "commit-gap", None, None, true);
    conn.execute("UPDATE backend_job_deliveries SET delivery_id='legacy-stable-request' WHERE delivery_id=?1",[&unknown]).unwrap();
    let unknown = "legacy-stable-request".to_string();
    let checker = job_notice_legacy_seed(&conn, "checker-unknown", Some("sending"), None, true);
    let mut checker_request: BackendJobRequest = serde_json::from_str(
        &conn
            .query_row(
                "SELECT request_json FROM backend_jobs WHERE job_id='checker-unknown'",
                [],
                |r| r.get::<_, String>(0),
            )
            .unwrap(),
    )
    .unwrap();
    checker_request.purpose = crate::ticket_item_checker::PURPOSE.into();
    checker_request.input = serde_json::json!({"ticket_id":"deleted-ticket","ticket_resource_key":"T-42","content_digest":"original-digest","title":"title","body":"Writer-only scope","edit":{"kind":"create"}});
    let checker_result = serde_json::json!({"findings":[{"category":"writer_scope","quote":"Writer-only scope","reason":"This is writer scope.","suggestion":"Remove it."}]});
    conn.execute(
        "UPDATE backend_jobs SET request_json=?1,result_json=?2 WHERE job_id='checker-unknown'",
        params![
            serde_json::to_string(&checker_request).unwrap(),
            checker_result.to_string()
        ],
    )
    .unwrap();
    conn.execute(
        "UPDATE backend_job_attempts SET result_json=?1 WHERE job_id='checker-unknown'",
        [checker_result.to_string()],
    )
    .unwrap();
    // Multiple retained identities must never create a fresh send: an ACK wins
    // over unknown history; without an ACK the ambiguity stays terminal.
    let multi_ack = job_notice_legacy_seed(&conn, "multi-ack", Some("completed"), None, true);
    let multi_unknown = job_notice_legacy_seed(&conn, "multi-unknown", Some("unknown"), Some("notification_outcome_unknown"), true);
    for id in [&multi_ack, &multi_unknown] {
        conn.execute("INSERT INTO backend_job_deliveries(workspace_id,delivery_id,job_id,attempt_id,target_runtime_id,target_worker_id,state,failure_category,created_at,updated_at) SELECT workspace_id,'another-' || delivery_id,job_id,attempt_id,target_runtime_id,target_worker_id,'unknown','notification_outcome_unknown','0','0' FROM backend_job_deliveries WHERE delivery_id=?1", [id]).unwrap();
    }
    // Earlier unknown beats the derived identity only when there is no real ACK.
    let multi_unknown_retained = format!("another-{multi_unknown}");
    let before = stored_fixture_rows(&conn, "backend_job_deliveries");
    // Force migration failure after the source parent rebuild and verify rollback
    // restores the old parent, rows, constraints, version and connection pragmas.
    conn.execute_batch("CREATE TABLE notification_job_targets(collision TEXT)")
        .unwrap();
    assert!(migrate_job_notifications_v95_to_v96(&conn).is_err());
    assert_eq!(current_schema_version(&conn).unwrap(), 95);
    assert_eq!(ticket_graph(&conn), ticket_before);
    assert_eq!(fixture_query_rows(&conn, "SELECT type,name,sql FROM sqlite_schema WHERE type IN ('trigger','index') AND sql IS NOT NULL ORDER BY name"), schema_before);
    check_guards(&conn);
    assert_eq!(stored_fixture_rows(&conn, "backend_job_deliveries"), before);
    assert_eq!(
        conn.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    conn.execute_batch("DROP TABLE notification_job_targets")
        .unwrap();
    migrate_job_notifications_v95_to_v96(&conn).unwrap();
    assert_eq!(current_schema_version(&conn).unwrap(), 96);
    assert_eq!(ticket_graph(&conn), ticket_before);
    check_guards(&conn);
    let schema_after = fixture_query_rows(&conn, "SELECT type,name,sql FROM sqlite_schema WHERE type IN ('trigger','index') AND sql IS NOT NULL ORDER BY name");
    assert!(schema_before.iter().all(|row| schema_after.contains(row)));
    assert_eq!(stored_fixture_rows(&conn, "backend_job_deliveries"), before);
    for (id, disposition, reason, runtime_accepted) in [
        (&accepted, "deliver", "result", true),
        (&clean, "suppress", "no_findings", false),
        (&stale, "suppress", "stale_content", false),
        (&unknown, "deliver", "result", false),
        (&ambiguous, "defer", "source_unavailable", false),
        (&multi_ack, "deliver", "result", true),
        (&multi_unknown_retained, "defer", "source_unavailable", false),
    ] {
        let got=conn.query_row("SELECT d.disposition,d.reason,l.runtime_accepted FROM notification_decisions d JOIN notification_job_legacy_receipts l USING(workspace_id,delivery_id) WHERE d.workspace_id='w' AND d.delivery_id=?1",[id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,bool>(2)?))).unwrap();
        assert_eq!(got, (disposition.into(), reason.into(), runtime_accepted));
    }
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM notification_acknowledgements",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1,
        "retain the Ticket ACK, never fabricate modern ACKs for legacy Job receipts"
    );
    for id in [&multi_ack, &multi_unknown_retained] {
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM notification_requests WHERE delivery_id=?1", [id], |r| r.get::<_, i64>(0)).unwrap(), 0);
    }
    assert_eq!(conn.query_row("SELECT code FROM notification_failures WHERE delivery_id=?1", [&multi_unknown_retained], |r| r.get::<_, String>(0)).unwrap(), "preparation_failed");
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );
    let pinned: String = conn
        .query_row(
            "SELECT content FROM notification_requests WHERE delivery_id=?1",
            [&unknown],
            |r| r.get(0),
        )
        .unwrap();
    assert!(pinned.contains("Durable result digest: digest."));
    drop(conn);
    let store = SqliteWorkspaceStore::open_for_test(path).unwrap();
    store.with_conn(|conn| {
        assert_eq!(ticket_graph(conn), ticket_before);
        assert_eq!(stored_fixture_rows(conn, "backend_job_deliveries"), before);
        check_guards(conn);
        Ok(())
    }).unwrap();
    let pending = store.list_pending_job_notifications("w", 100).unwrap();
    assert_eq!(pending.len(), 4);
    let checker_body = store
        .job_notification_request("w", &checker)
        .unwrap()
        .unwrap()
        .0;
    assert!(checker_body.contains("Content digest: original-digest"));
    assert!(checker_body.contains("Quote: \"Writer-only scope\""));
    assert!(checker_body.contains("not a new user request, approval gate"));
    assert!(pending.iter().all(|s| s.delivery_id != accepted
        && s.delivery_id != clean
        && s.delivery_id != stale
        && s.delivery_id != ambiguous
        && s.delivery_id != multi_ack
        && s.delivery_id != multi_unknown_retained
        && s.delivery_id != multi_unknown));
    assert_eq!(
        store
            .job_notification_request("w", &unknown)
            .unwrap()
            .unwrap()
            .0,
        pinned
    );
}

#[tokio::test]
async fn job_notification_source_is_atomic_with_result_and_schema_guards_are_verified() {
    let (directory, store) = tracking_store_fixture().await;
    let mut request = resource_job_request("atomic-source", "atomic-source", "atomic-source");
    request.purpose = "notification_test".into();
    request.grants = Default::default();
    let worker = RuntimeWorkerRef::new("r", "replacement");
    let reservation = store
        .reserve_backend_job("w", &request, RESOURCE_JOB_NOW)
        .unwrap();
    store
        .claim_backend_job_attempt_dispatch(
            "w",
            &request.job_id,
            &reservation.attempt.attempt_id,
            RESOURCE_JOB_NOW,
        )
        .unwrap();
    store
        .bind_backend_job_attempt_worker(
            "w",
            &request.job_id,
            &reservation.attempt.attempt_id,
            &worker,
            Some("run"),
            RESOURCE_JOB_NOW,
        )
        .unwrap();
    let submission = BackendJobResultSubmission {
        job_id: request.job_id.clone(),
        attempt_id: reservation.attempt.attempt_id.clone(),
        input_digest: request.input_digest().unwrap(),
        result: serde_json::json!({"saved":true}),
    };
    store.with_conn(|conn| {conn.execute_batch("CREATE TRIGGER reject_job_source BEFORE INSERT ON notification_sources WHEN NEW.role='backend_job' BEGIN SELECT RAISE(ABORT,'injected source commit failure'); END;")?;Ok(())}).unwrap();
    assert!(
        store
            .accept_backend_job_result("w", &worker, &submission, RESOURCE_JOB_NOW)
            .is_err()
    );
    assert_eq!(
        store
            .get_backend_job("w", &request.job_id)
            .unwrap()
            .unwrap()
            .state,
        BackendJobState::Pending
    );
    assert!(
        store
            .list_pending_job_notifications("w", 100)
            .unwrap()
            .is_empty()
    );
    store
        .with_conn(|conn| {
            conn.execute_batch("DROP TRIGGER reject_job_source")?;
            Ok(())
        })
        .unwrap();
    store
        .accept_backend_job_result("w", &worker, &submission, RESOURCE_JOB_NOW)
        .unwrap();
    assert_eq!(
        store
            .list_pending_job_notifications("w", 100)
            .unwrap()
            .len(),
        1
    );
    store
        .with_conn(|conn| {
            conn.execute_batch("DROP TRIGGER notification_job_targets_no_update")?;
            Ok(())
        })
        .unwrap();
    drop(store);
    assert!(
        matches!(SqliteWorkspaceStore::open_for_test(directory.path().join("workspace.db")),Err(error) if error.to_string().contains("notification_job_targets_no_update"))
    );
}
