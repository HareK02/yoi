// Included in worker_messaging_connected_tests: real signed WorkerSpawn, Runtime,
// Controller, Session store and common delivery ledger. Only model output/gates
// are substituted. The child never calls a notification or result-report Tool.
#[derive(Clone)]
struct ActivityModel {
    policy: Arc<Mutex<Option<server_api::launch_policy::WorkerLaunchPolicy>>>,
    spawned: tokio::sync::mpsc::Sender<RuntimeWorkerRef>,
    child_started: tokio::sync::mpsc::Sender<()>,
    observed: tokio::sync::mpsc::Sender<()>,
    release_child: Arc<tokio::sync::Semaphore>,
    release_parent: Arc<tokio::sync::Semaphore>,
    parent_busy: bool,
    child_error: bool,
}
#[async_trait::async_trait]
impl agen::llm_client::LlmClient for ActivityModel {
    fn clone_boxed(&self) -> Box<dyn agen::llm_client::LlmClient> {
        Box::new(self.clone())
    }
    async fn stream(
        &self,
        request: agen::llm_client::Request,
    ) -> std::result::Result<agen::llm_client::client::ResponseStream, agen::llm_client::ClientError>
    {
        let messages = request
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Message { content, .. } => Some(serde_json::to_string(content).unwrap()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let events = if messages.contains("ACTIVITY_PARENT") {
            if messages.contains("committed activity") {
                let _ = self.observed.try_send(());
                done()
            } else if let Some(child) = result(&request, "activity-spawn") {
                self.spawned
                    .try_send(RuntimeWorkerRef::new(
                        child["runtime_id"].as_str().unwrap(),
                        child["worker_id"].as_str().unwrap(),
                    ))
                    .unwrap();
                if self.parent_busy {
                    self.release_parent.acquire().await.unwrap().forget();
                }
                done()
            } else {
                let policy = self.policy.lock().unwrap().clone().unwrap();
                call(
                    "activity-spawn",
                    "WorkerSpawn",
                    json!({"runtime_id":CONNECTED_RUNTIME,"profile":"project:messaging-peer","operation_id":"automatic-activity-spawn","display_name":"Silent lifecycle child","initial_submit":[{"kind":"text","content":"ACTIVITY_CHILD"}],"authorization":{"kind":"policy","policy_id":policy.policy_id,"policy_authorization_key":policy.authorization_key}}),
                )
            }
        } else {
            assert!(messages.contains("ACTIVITY_CHILD"));
            self.child_started.try_send(()).unwrap();
            self.release_child.acquire().await.unwrap().forget();
            if self.child_error {
                return Err(agen::llm_client::ClientError::Api {
                    status: Some(400),
                    code: Some("activity_model_rejected".into()),
                    message: "private-provider-error-must-not-be-notified".into(),
                    retry_after: None,
                });
            }
            done()
        };
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}

async fn activity_wait_idle(h: &ConnectedWorkerHarness, worker: &RuntimeWorkerRef) {
    use protocol::subscription::{
        EventSubscriptionSelector, SubscriptionEventPayload, SubscriptionSnapshot,
        SubscriptionWorkerState,
    };
    let mut events = h
        .runtime
        .subscribe_event_selector(EventSubscriptionSelector::RuntimeWorkers)
        .unwrap();
    if let SubscriptionSnapshot::Workers { workers } = events.snapshot() {
        if workers.iter().any(|w| {
            w.worker_id.to_string() == worker.worker_id && w.state == SubscriptionWorkerState::Idle
        }) {
            return;
        }
    }
    loop {
        if let SubscriptionEventPayload::WorkerUpserted { worker: w } =
            events.recv().await.unwrap().payload
        {
            if w.worker_id.to_string() == worker.worker_id
                && w.state == SubscriptionWorkerState::Idle
            {
                return;
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActivityCase {
    Normal,
    ModelError,
    Cancel,
    RunningStop,
    ParentStopped,
    LostAck,
    SourceFailure,
    RelationRevoked,
    BodyReplaced,
    AutomaticHook,
}

async fn automatic_activity_delivery(parent_busy: bool, case: ActivityCase) {
    let (spawned, mut spawn_rx) = tokio::sync::mpsc::channel(2);
    let (child_started, mut child_ready) = tokio::sync::mpsc::channel(4);
    let (observed, mut observed_rx) = tokio::sync::mpsc::channel(4);
    let policy = Arc::new(Mutex::new(None));
    let release_child = Arc::new(tokio::sync::Semaphore::new(0));
    let release_parent = Arc::new(tokio::sync::Semaphore::new(0));
    let h = ConnectedWorkerHarness::start(ActivityModel {
        policy: policy.clone(),
        spawned,
        child_started,
        observed,
        release_child: release_child.clone(),
        release_parent: release_parent.clone(),
        parent_busy,
        child_error: case == ActivityCase::ModelError,
    })
    .await;
    let api = &h.api;
    install_messaging_profiles(api);
    let subject = open_subjektiv_store(api)
        .unwrap()
        .create_subject(crate::subjektiv::SubjectRole::new("activity-parent").unwrap())
        .unwrap()
        .id;
    let child_assignment = key_assignment(&["worker.list"]);
    let launch = seed_owner_launch_policy(
        api,
        &h.owner,
        "activity",
        &["project:messaging-peer"],
        child_assignment.clone(),
    )
    .await;
    let mut keys = vec![
        "session.record",
        "memory.resident.read",
        "memory.resident.supply",
        "journal.startup.read",
        "journal.startup.supply",
        "worker.create",
        "worker.observe",
        "worker.messaging",
        "worker.stop",
    ];
    let usage = launch.usage_key().unwrap();
    keys.push(usage.as_str());
    super::chat_tests::replace_subject_policy(
        api,
        &h.owner,
        &subject,
        key_assignment(&keys),
        child_assignment,
    )
    .await;
    *policy.lock().unwrap() = Some(launch);
    let Json(created) =
        create_workspace_worker_as_owner(
            State(api.clone()),
            HeaderMap::new(),
            Json(CreateWorkspaceWorkerRequest {
                runtime_id: CONNECTED_RUNTIME.into(),
                display_name: "Activity parent".into(),
                singleton_key: None,
                profile: Some("project:messaging-origin".into()),
                initial_submit: vec![protocol::Segment::Text {
                    content: "ACTIVITY_PARENT".into(),
                }],
                workdir_attachments: vec![],
                feature_connections:
                    crate::server::subjektiv_worker_initialization::subject_connection(subject.into())
                        .unwrap(),
                control_operation_id: Some(Uuid::now_v7().to_string()),
                authorization: server_api::launch_policy::WorkerCreationAuthorization::Assignment {
                    assignment: None,
                },
            }),
        )
        .await
        .unwrap();
    let parent = RuntimeWorkerRef::new(&created.runtime_id, &created.worker_id);
    let child = spawn_rx.recv().await.expect("public spawn result");
    if !parent_busy {
        activity_wait_idle(&h, &parent).await;
    }
    child_ready
        .recv()
        .await
        .expect("child model entered its running gate");
    if matches!(
        case,
        ActivityCase::Cancel | ActivityCase::RunningStop | ActivityCase::ParentStopped
    ) {
        let runtime = api.runtime.clone();
        let target = if case == ActivityCase::ParentStopped {
            parent.clone()
        } else {
            child.clone()
        };
        api.host_executors
            .host()
            .unwrap()
            .run(move || {
                if case == ActivityCase::Cancel {
                    runtime
                        .cancel_worker(&target, WorkerLifecycleRequest::default())
                        .unwrap();
                } else {
                    runtime
                        .stop_worker(&target, WorkerLifecycleRequest::default())
                        .unwrap();
                }
                Ok::<(), crate::Error>(())
            })
            .await
            .unwrap();
    }
    if !matches!(case, ActivityCase::Cancel | ActivityCase::RunningStop) {
        release_child.add_permits(1);
    }
    if case != ActivityCase::RunningStop {
        activity_wait_idle(&h, &child).await;
    }
    if matches!(
        case,
        ActivityCase::RelationRevoked | ActivityCase::BodyReplaced
    ) {
        api.config_store.with_conn(|conn| {
            conn.execute_batch("CREATE TRIGGER activity_hold_delivery BEFORE INSERT ON notification_requests BEGIN SELECT RAISE(ABORT,'hold before runtime send'); END;")?;
            Ok(())
        }).unwrap();
    }
    if case == ActivityCase::LostAck {
        api.config_store.with_conn(|conn| {
            conn.execute_batch("CREATE TRIGGER activity_fail_ack BEFORE INSERT ON notification_acknowledgements BEGIN SELECT RAISE(ABORT,'injected ACK loss'); END;")?;
            Ok(())
        }).unwrap();
    }
    if case == ActivityCase::SourceFailure {
        api.config_store.with_conn(|conn| {
            conn.execute_batch("CREATE TRIGGER activity_fail_source BEFORE INSERT ON notification_sources WHEN NEW.role='worker_lifecycle' BEGIN SELECT RAISE(ABORT,'injected source failure'); END;")?;
            Ok(())
        }).unwrap();
        let recovery = api.clone();
        api.host_executors
            .host()
            .unwrap()
            .run(move || {
                worker_lifecycle_notifications::recover(&recovery);
                assert!(
                    recovery
                        .store
                        .pending_worker_lifecycle(TEST_WORKSPACE_ID, 20)?
                        .is_empty()
                );
                recovery.config_store.with_conn(|conn| {
                    conn.execute_batch("DROP TRIGGER activity_fail_source")?;
                    Ok(())
                })?;
                Ok::<(), crate::Error>(())
            })
            .await
            .unwrap();
    }
    if case == ActivityCase::AutomaticHook {
        // Run the actual production subscription/timer owner. No explicit scan,
        // child reporting Tool, or user input is allowed before parent observation.
        let hook = run_orchestrator_turn_end_hook(api.clone());
        tokio::pin!(hook);
        tokio::select! {
            _ = &mut hook => panic!("recovery owner unexpectedly ended"),
            notice = observed_rx.recv() => { notice.expect("automatic recovery reached parent model"); }
        }
        activity_wait_idle(&h, &parent).await;
    }
    // Invoke the production recovery callback; its periodic scheduling is separate
    // from the persisted-source→signed Runtime→Controller connection under test.
    let recovery = api.clone();
    let scan_child = child.clone();
    api.host_executors.host().unwrap().run(move || {
        let scans = recovery.store.lifecycle_scan_targets(TEST_WORKSPACE_ID,8)?;
        assert_eq!(scans.len(),1,"spawn relation must be a recovery target: {scans:?}");
        let activity = recovery.runtime.worker_activity(&scan_child.runtime_id,runtime_api::WorkerActivityRequest {workspace_id:TEST_WORKSPACE_ID.into(),worker_id:scan_child.worker_id,run_cursor:None,stop_after:None}).expect("signed activity source read");
        assert!(matches!(&activity.run,runtime_api::RunTerminalAvailability::Page {page} if !page.facts.is_empty()) || (case == ActivityCase::RunningStop && !activity.stops.is_empty()),"committed activity fact: {activity:?}");
        worker_lifecycle_notifications::recover(&recovery);Ok::<(), crate::Error>(())
    }).await.unwrap();
    let diagnostics = api
        .store
        .notification_diagnostics(
            TEST_WORKSPACE_ID,
            &server_api::NotificationDiagnosticsQuery::default(),
        )
        .unwrap();
    let lifecycle: Vec<_> = diagnostics
        .items
        .iter()
        .filter(|i| i.recipient_role == "worker_lifecycle")
        .collect();
    if matches!(
        case,
        ActivityCase::RelationRevoked | ActivityCase::BodyReplaced
    ) {
        assert_eq!(lifecycle.len(), 1, "{diagnostics:?}");
        let pending = api
            .store
            .pending_worker_lifecycle(TEST_WORKSPACE_ID, 20)
            .unwrap();
        assert_eq!(pending.len(), 1);
        if case == ActivityCase::RelationRevoked {
            api.store
                .revoke_worker_control_grant(
                    TEST_WORKSPACE_ID,
                    &pending[0].grant_id,
                    &now_registry_timestamp(),
                )
                .unwrap();
        } else {
            // Replace only the authoritative owner fixture, not the delivery or
            // authorization implementations. The new body has no child-observe grant.
            let lease = api
                .store
                .retained_worker_singleton_lease(TEST_WORKSPACE_ID, &parent)
                .unwrap()
                .unwrap();
            api.config_store.with_conn(|conn| {
                let changed = conn.execute("UPDATE worker_singleton_owners SET runtime_id=?3,worker_id=?4,generation=generation+1 WHERE workspace_id=?1 AND singleton_key=?2", rusqlite::params![TEST_WORKSPACE_ID, lease.key, child.runtime_id, child.worker_id])?;
                assert_eq!(changed, 1);
                Ok(())
            }).unwrap();
        }
        api.config_store
            .with_conn(|conn| {
                conn.execute_batch("DROP TRIGGER activity_hold_delivery")?;
                Ok(())
            })
            .unwrap();
        let recovery = api.clone();
        api.host_executors
            .host()
            .unwrap()
            .run(move || {
                worker_lifecycle_notifications::recover(&recovery);
                Ok::<(), crate::Error>(())
            })
            .await
            .unwrap();
        assert!(
            observed_rx.try_recv().is_err(),
            "must not deliver to old body or revoked parent"
        );
        let still_pending = api
            .store
            .pending_worker_lifecycle(TEST_WORKSPACE_ID, 20)
            .unwrap();
        assert_eq!(still_pending.len(), 1);
        assert_eq!(still_pending[0].delivery_id, pending[0].delivery_id);
        for worker in [&parent, &child] {
            let worker_ref = worker_runtime::identity::WorkerRef::try_from(worker).unwrap();
            let snapshot = h.factory.retained_session_snapshot(&worker_ref).unwrap();
            assert!(!snapshot.snapshot.entries.iter().any(|entry| {
                serde_json::to_string(entry)
                    .unwrap()
                    .contains("committed activity")
            }));
        }
        return;
    }
    if case == ActivityCase::ParentStopped {
        assert_eq!(lifecycle.len(), 1, "{diagnostics:?}");
        assert_ne!(
            lifecycle[0].stage,
            server_api::NotificationDiagnosticStage::RuntimeAccepted
        );
        assert!(observed_rx.try_recv().is_err());
        assert_eq!(
            api.store
                .pending_worker_lifecycle(TEST_WORKSPACE_ID, 20)
                .unwrap()
                .len(),
            1
        );
        let runtime = api.runtime.clone();
        api.host_executors
            .host()
            .unwrap()
            .run(move || {
                assert_eq!(
                    runtime.worker(&parent).unwrap().state,
                    "stopped",
                    "notification must not restore parent"
                );
                Ok::<(), crate::Error>(())
            })
            .await
            .unwrap();
        return;
    }
    if case == ActivityCase::RunningStop {
        assert!(!lifecycle.is_empty(), "{diagnostics:?}");
        assert!(
            lifecycle
                .iter()
                .all(|i| i.stage == server_api::NotificationDiagnosticStage::RuntimeAccepted)
        );
        observed_rx
            .recv()
            .await
            .expect("running stop reached parent model");
        activity_wait_idle(&h, &parent).await;
        let parent_ref = worker_runtime::identity::WorkerRef::try_from(&parent).unwrap();
        let snapshot = h.factory.retained_session_snapshot(&parent_ref).unwrap();
        let notifications: Vec<_> = snapshot
            .snapshot
            .entries
            .iter()
            .map(|e| serde_json::to_string(e).unwrap())
            .filter(|s| s.contains("committed activity"))
            .collect();
        assert!(
            notifications.iter().any(|s| s.contains("Stopped")),
            "{notifications:?}"
        );
        assert!(
            !notifications
                .iter()
                .any(|s| s.contains("activity Completed")),
            "{notifications:?}"
        );
        return;
    }
    assert_eq!(lifecycle.len(), 1, "{diagnostics:?}");
    if case == ActivityCase::LostAck {
        assert_ne!(
            lifecycle[0].stage,
            server_api::NotificationDiagnosticStage::RuntimeAccepted
        );
        api.config_store
            .with_conn(|conn| {
                conn.execute_batch("DROP TRIGGER activity_fail_ack")?;
                Ok(())
            })
            .unwrap();
    } else {
        assert_eq!(
            lifecycle[0].stage,
            server_api::NotificationDiagnosticStage::RuntimeAccepted,
            "{diagnostics:?}"
        );
    }
    if parent_busy {
        assert!(
            observed_rx.try_recv().is_err(),
            "Busy model must not be interrupted by notification"
        );
        release_parent.add_permits(1);
    }
    if case != ActivityCase::AutomaticHook {
        observed_rx
            .recv()
            .await
            .expect("committed notification reached parent model");
    }
    activity_wait_idle(&h, &parent).await;
    let source = lifecycle[0].source_event_id.clone();
    for _ in 0..2 {
        let recovery = api.clone();
        api.host_executors
            .host()
            .unwrap()
            .run(move || {
                worker_lifecycle_notifications::recover(&recovery);
                Ok::<(), crate::Error>(())
            })
            .await
            .unwrap();
    }
    let parent_ref = worker_runtime::identity::WorkerRef::try_from(&parent).unwrap();
    let snapshot = h.factory.retained_session_snapshot(&parent_ref).unwrap();
    assert_eq!(
        snapshot
            .snapshot
            .entries
            .iter()
            .filter(|e| serde_json::to_string(e)
                .unwrap()
                .contains("committed activity"))
            .count(),
        1,
        "same source must not append duplicate parent input"
    );
    let child_ref = worker_runtime::identity::WorkerRef::try_from(&child).unwrap();
    let child_history = serde_json::to_string(
        &h.factory
            .retained_session_snapshot(&child_ref)
            .unwrap()
            .snapshot,
    )
    .unwrap();
    assert!(!child_history.contains("\"name\":\"WorkerNotify\""));
    let diagnostic = api
        .store
        .notification_diagnostics(
            TEST_WORKSPACE_ID,
            &server_api::NotificationDiagnosticsQuery {
                source_event_id: Some(source),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(diagnostic.items.len(), 1);
    assert!(
        !diagnostic.items[0].model_processing_confirmed,
        "diagnostics must not invent the separate model evidence"
    );

    if case != ActivityCase::Normal {
        let notifications: Vec<_> = snapshot
            .snapshot
            .entries
            .iter()
            .map(|entry| serde_json::to_string(entry).unwrap())
            .filter(|text| text.contains("committed activity"))
            .collect();
        let expected = match case {
            ActivityCase::ModelError => "Errored",
            ActivityCase::Cancel => "Cancelled",
            _ => "Completed",
        };
        assert!(
            notifications[0].contains(&format!("activity {expected}")),
            "{notifications:?}"
        );
        assert!(!notifications[0].contains("private-provider-error"));
        assert_eq!(
            diagnostic.items[0].stage,
            server_api::NotificationDiagnosticStage::RuntimeAccepted
        );
        return;
    }

    // A later Submit creates a distinct terminal, not another delivery of run one.
    let runtime = api.runtime.clone();
    let continued_child = child.clone();
    api.host_executors
        .host()
        .unwrap()
        .run(move || {
            runtime
                .send_input(
                    &continued_child,
                    WorkerInputRequest {
                        source: None,
                        kind: WorkerInputKind::UserIfIdle,
                        content: "ACTIVITY_CHILD continued".into(),
                        submission_request_id: Some("activity-continuation".into()),
                        segments: None,
                    },
                )
                .expect("signed continuation");
            Ok::<(), crate::Error>(())
        })
        .await
        .unwrap();
    child_ready
        .recv()
        .await
        .expect("continued child reached model");
    release_child.add_permits(1);
    activity_wait_idle(&h, &child).await;
    let recovery = api.clone();
    api.host_executors
        .host()
        .unwrap()
        .run(move || {
            worker_lifecycle_notifications::recover(&recovery);
            Ok::<(), crate::Error>(())
        })
        .await
        .unwrap();
    observed_rx
        .recv()
        .await
        .expect("continuation terminal reached parent model");
    activity_wait_idle(&h, &parent).await;

    // Idle Stop is a third execution fact; no second run is invented by Stop.
    let runtime = api.runtime.clone();
    let stopped_child = child.clone();
    api.host_executors
        .host()
        .unwrap()
        .run(move || {
            runtime
                .stop_worker(&stopped_child, WorkerLifecycleRequest::default())
                .expect("signed stop");
            Ok::<(), crate::Error>(())
        })
        .await
        .unwrap();
    let recovery = api.clone();
    api.host_executors
        .host()
        .unwrap()
        .run(move || {
            worker_lifecycle_notifications::recover(&recovery);
            Ok::<(), crate::Error>(())
        })
        .await
        .unwrap();
    observed_rx
        .recv()
        .await
        .expect("confirmed stop reached parent model");
    activity_wait_idle(&h, &parent).await;
    let diagnostics = api
        .store
        .notification_diagnostics(
            TEST_WORKSPACE_ID,
            &server_api::NotificationDiagnosticsQuery::default(),
        )
        .unwrap();
    let lifecycle: Vec<_> = diagnostics
        .items
        .iter()
        .filter(|i| i.recipient_role == "worker_lifecycle")
        .collect();
    assert_eq!(
        lifecycle.len(),
        3,
        "initial run, continuation and stop: {diagnostics:?}"
    );
    assert!(
        lifecycle
            .iter()
            .all(|i| i.stage == server_api::NotificationDiagnosticStage::RuntimeAccepted)
    );
    let snapshot = h.factory.retained_session_snapshot(&parent_ref).unwrap();
    assert_eq!(
        snapshot
            .snapshot
            .entries
            .iter()
            .filter(|e| serde_json::to_string(e)
                .unwrap()
                .contains("committed activity"))
            .count(),
        3
    );
}

macro_rules! activity_fault_test {
    ($name:ident, $case:ident) => {
        #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
        async fn $name() {
            tokio::time::timeout(
                std::time::Duration::from_secs(120),
                automatic_activity_delivery(false, ActivityCase::$case),
            )
            .await
            .expect("bounded fault lifecycle delivery");
        }
    };
}
activity_fault_test!(
    worker_lifecycle_automatic_recovery_owner_delivers_without_manual_scan,
    AutomaticHook
);
activity_fault_test!(
    worker_lifecycle_model_error_reaches_parent_without_private_error,
    ModelError
);
activity_fault_test!(
    worker_lifecycle_cancel_before_model_output_reaches_parent,
    Cancel
);
activity_fault_test!(
    worker_lifecycle_running_stop_reaches_parent_without_success,
    RunningStop
);
activity_fault_test!(
    worker_lifecycle_stopped_parent_retains_source_without_restore,
    ParentStopped
);
activity_fault_test!(
    worker_lifecycle_lost_ack_retries_without_duplicate_parent_input,
    LostAck
);
activity_fault_test!(
    worker_lifecycle_source_failure_recovers_without_advancing_cursor,
    SourceFailure
);
activity_fault_test!(
    worker_lifecycle_revocation_after_capture_prevents_delivery,
    RelationRevoked
);
activity_fault_test!(
    worker_lifecycle_body_replacement_never_delivers_to_old_body_or_transfers_rights,
    BodyReplaced
);

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn worker_lifecycle_silent_child_notifies_idle_subject_through_real_controller() {
    tokio::time::timeout(
        std::time::Duration::from_secs(120),
        automatic_activity_delivery(false, ActivityCase::Normal),
    )
    .await
    .expect("bounded connected lifecycle delivery");
}
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn worker_lifecycle_silent_child_is_retained_while_subject_is_busy() {
    tokio::time::timeout(
        std::time::Duration::from_secs(120),
        automatic_activity_delivery(true, ActivityCase::Normal),
    )
    .await
    .expect("bounded connected lifecycle delivery");
}
