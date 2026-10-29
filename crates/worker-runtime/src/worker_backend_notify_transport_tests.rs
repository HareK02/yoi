// Concrete Runtime -> Controller -> filesystem receipt/history tests. Only model
// I/O is gated; the Runtime adapter and notification acceptance owner are real.
mod notify_transport {
    use super::*;

    const WAIT: Duration = Duration::from_secs(5);

    async fn accepted(events: &mut tokio::sync::broadcast::Receiver<Event>, id: &str) {
        tokio::time::timeout(WAIT, async {
            loop {
                match events.recv().await.unwrap() {
                    Event::NotificationAccepted {
                        notification_request_id,
                    } if notification_request_id == id => return,
                    Event::NotificationRejected { message, .. } | Event::Error { message, .. } => {
                        panic!("notification rejected: {message}")
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("durable notification acceptance");
    }

    #[test]
    fn busy_notify_retains_source_scoped_receipts_and_commits_provenance_before_next_model_input() {
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let next_started = Arc::new(tokio::sync::Notify::new());
        let next_release = Arc::new(tokio::sync::Notify::new());
        let client = MockClient::sequential(vec![
            MockResponse::Gated {
                started: started.clone(),
                release: release.clone(),
                events: simple_text_events(),
            },
            MockResponse::Gated {
                started: next_started.clone(),
                release: next_release.clone(),
                events: simple_text_events(),
            },
        ]);
        let runtime_base = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let backend = Arc::new(
            WorkerRuntimeExecutionBackend::new(MockFactory {
                client: client.clone(),
                runtime_base: runtime_base.path().to_path_buf(),
                cwd: cwd.path().to_path_buf(),
                store_dir: store.path().join("sessions"),
                worker_metadata_dir: store.path().join("workers"),
                observed_cwds: Arc::new(Mutex::new(Vec::new())),
                observed_workspace_clients: Arc::new(Mutex::new(Vec::new())),
            })
            .unwrap(),
        );
        let runtime =
            EmbeddedRuntime::with_execution_backend(RuntimeOptions::default(), backend.clone())
                .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let detail = runtime
            .create_worker(create_request("notify-provenance"))
            .unwrap();
        let worker_ref = &detail.worker_ref;
        let handle = backend.workers.lock().unwrap()[worker_ref].handle.clone();
        runtime
            .send_input(worker_ref, WorkerInput::user("original request"))
            .unwrap();
        backend
            .run_on_adapter_runtime(async move {
                tokio::time::timeout(WAIT, started.notified())
                    .await
                    .unwrap();
                Ok(())
            })
            .unwrap();
        let account = protocol::AuthenticatedInputSource::Account {
            account_id: "account-a".into(),
        };
        let mut human = WorkerInput::notify("human-notice-body");
        human.source = Some(account.clone());
        human.submission_request_id = Some("same-client-id".into());

        // Simulate an ACK lost above the Controller acceptance boundary. The
        // sender retries through Runtime, using the same source, ID and body.
        let lost_handle = handle.clone();
        backend
            .run_on_adapter_runtime(async move {
                let mut events = lost_handle.subscribe();
                lost_handle
                    .send(Method::NotifyTracked {
                        notification_request_id: "same-client-id".into(),
                        message: "human-notice-body".into(),
                        source: account,
                    })
                    .await
                    .unwrap();
                accepted(&mut events, "same-client-id").await;
                Ok(())
            })
            .unwrap();
        let first = runtime.send_input(worker_ref, human.clone()).unwrap();
        let retry = runtime.send_input(worker_ref, human.clone()).unwrap();
        assert_eq!(first.notification, retry.notification);
        assert!(first.submission.is_none());
        let mut conflict = human.clone();
        conflict.content = "changed-body".into();
        assert!(runtime.send_input(worker_ref, conflict).is_err());

        let mut other_account = human.clone();
        other_account.source = Some(protocol::AuthenticatedInputSource::Account {
            account_id: "account-b".into(),
        });
        other_account.content = "other-account-body".into();
        runtime.send_input(worker_ref, other_account).unwrap();
        let mut report = human.clone();
        report.source = Some(protocol::AuthenticatedInputSource::Worker {
            runtime_id: "report-runtime".into(),
            worker_id: "report-worker".into(),
        });
        report.content = "worker-report-body".into();
        runtime.send_input(worker_ref, report).unwrap();
        let mut system = WorkerInput::notify("system-notice-body");
        system.submission_request_id = Some("system-notice-id".into());
        runtime.send_input(worker_ref, system).unwrap();
        assert_eq!(handle.shared_state.catalog_status(), WorkerStatus::Running);
        assert_eq!(
            client.call_count.load(Ordering::SeqCst),
            1,
            "notify must not interrupt the current model stream"
        );
        let before = handle.committed_entries();
        let checkpoint = before
            .iter()
            .rev()
            .find_map(|entry| match entry {
                LogEntry::Extension {
                    domain, payload, ..
                } if domain == "worker.pending_activations.v1" => Some(payload),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            checkpoint["notification_receipts"]
                .as_array()
                .unwrap()
                .len(),
            4
        );
        assert_eq!(
            checkpoint["pending_notifications"].as_array().unwrap().len()
                + checkpoint["activating_notification"].as_array().unwrap().len(),
            4
        );
        let sources = checkpoint["notification_receipts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|receipt| receipt["source_namespace"].as_str().unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            sources,
            BTreeSet::from([
                "account:account-a",
                "account:account-b",
                "worker:report-runtime:report-worker",
                "backend:system-notice-id",
            ])
        );
        release.notify_one();
        backend
            .run_on_adapter_runtime(async move {
                tokio::time::timeout(WAIT, next_started.notified())
                    .await
                    .unwrap();
                Ok(())
            })
            .unwrap();
        let entries = handle.committed_entries();
        let history = session_store::collect_state(&entries).annotated_history;
        let expected = [
            (
                "human-notice-body",
                LoggedSessionHistoryOrigin::HumanInput {
                    account_id: "account-a".into(),
                },
            ),
            (
                "other-account-body",
                LoggedSessionHistoryOrigin::HumanInput {
                    account_id: "account-b".into(),
                },
            ),
            (
                "worker-report-body",
                LoggedSessionHistoryOrigin::WorkerInput {
                    actor: session_store::LoggedWorkerSubject {
                        workspace_id: None,
                        runtime_id: Some("report-runtime".into()),
                        worker_id: "report-worker".into(),
                    },
                },
            ),
            (
                "system-notice-body",
                LoggedSessionHistoryOrigin::BackendInstruction {
                    operation_id: Some("system-notice-id".into()),
                },
            ),
        ];
        let requests = client.captured.lock().unwrap();
        let first_items = serde_json::to_string(&requests[0].items).unwrap();
        let next_items = serde_json::to_string(&requests[1].items).unwrap();
        for (body, origin) in expected {
            assert!(!first_items.contains(body));
            assert_eq!(next_items.matches(body).count(), 1);
            let matching = history
                .iter()
                .filter(|entry| serde_json::to_string(&entry.item).unwrap().contains(body))
                .collect::<Vec<_>>();
            assert_eq!(
                matching.len(),
                1,
                "notification must commit exactly once: {body}"
            );
            assert_eq!(matching[0].metadata.origin, origin);
        }
        drop(requests);
        assert!(!next_items.contains("changed-body"));
        // Provenance is annotation, not a grant or promoted Host instruction.
        let replay = runtime.send_input(worker_ref, human).unwrap();
        assert_eq!(first.notification, replay.notification);
        assert_eq!(
            serde_json::to_value(handle.committed_entries()).unwrap(),
            serde_json::to_value(entries).unwrap(),
            "retained receipt replay must not append history"
        );
        next_release.notify_one();
        runtime
            .stop_worker(worker_ref, Some("test complete".into()))
            .unwrap();
        assert!(
            runtime
                .send_input(worker_ref, WorkerInput::notify("stopped-notice"))
                .is_err()
        );
    }
}
