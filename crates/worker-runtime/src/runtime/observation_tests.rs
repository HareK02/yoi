#[cfg(feature = "ws-server")]
mod observation_sync_tests {
    use super::*;
    use crate::catalog::WorkerObservationState;
    use crate::execution::WorkerObservationPublication;

    #[test]
    fn internal_observation_reconnect_and_queued_events_preserve_child_source_order() {
        let mut activity = InternalWorkerActivityProjection::default();
        let worker = internal_worker_ref("child", None);
        let finished = protocol::WorkerStateSnapshot {
            state_sequence: 4,
            ..protocol::WorkerStateSnapshot::initial()
        };
        RuntimeState::update_internal_worker_activity(
            &mut activity,
            &protocol::Event::InternalWorker {
                worker: worker.clone(),
                event: Box::new(protocol::Event::WorkerState {
                    snapshot: finished.clone(),
                }),
            },
        );
        let mut older =
            internal_worker_snapshot(worker.clone(), protocol::WorkerStatus::Running, vec![]);
        older.state = Some(running(3));
        RuntimeState::update_internal_worker_activity(&mut activity, &parent_snapshot(vec![older]));
        RuntimeState::update_internal_worker_activity(
            &mut activity,
            &protocol::Event::InternalWorker {
                worker: worker.clone(),
                event: Box::new(protocol::Event::WorkerState {
                    snapshot: running(2),
                }),
            },
        );
        assert_eq!(activity.workers["child"].state_sequence, Some(4));
        assert_eq!(
            activity.workers["child"]
                .snapshot
                .as_ref()
                .map(protocol::WorkerStateSnapshot::catalog_status),
            Some(protocol::WorkerStatus::Idle)
        );
        assert!(!activity.has_running_worker());

        let mut unavailable =
            internal_worker_snapshot(worker, protocol::WorkerStatus::Running, vec![]);
        unavailable.state = None;
        RuntimeState::update_internal_worker_activity(
            &mut activity,
            &parent_snapshot(vec![unavailable]),
        );
        assert_eq!(
            activity.workers["child"]
                .snapshot
                .as_ref()
                .map(protocol::WorkerStateSnapshot::catalog_status),
            None
        );
        assert_eq!(activity.workers["child"].state_sequence, Some(4));
    }

    #[test]
    fn published_parent_snapshots_preserve_nested_completion_and_recovery_fences() {
        // Real execution-owned publication and cursor replay; only transport is
        // replaced. Assert the public payload, not merely Runtime's running flag.
        let (runtime, backend) = runtime_and_backend();
        let created = runtime
            .create_worker(task_request("nested public overlap"))
            .unwrap();
        let owner = context(&backend, &created.worker_ref);
        let child = internal_worker_ref("child", None);
        let grandchild = internal_worker_ref("grandchild", Some("child"));
        let mut nested =
            internal_worker_snapshot(grandchild.clone(), protocol::WorkerStatus::Running, vec![]);
        nested.state = Some(running(7));
        let mut older =
            internal_worker_snapshot(child.clone(), protocol::WorkerStatus::Running, vec![nested]);
        older.state = Some(running(3));
        let _ = owner
            .publish_protocol_event(parent_snapshot(vec![older.clone()]))
            .unwrap();
        let finished = |sequence, id: &str| protocol::WorkerStateSnapshot {
            state_sequence: sequence,
            last_finished_submission_request_id: Some(id.into()),
            ..protocol::WorkerStateSnapshot::initial()
        };
        let child_finished = finished(4, "child-finished");
        let grandchild_finished = finished(8, "grandchild-finished");
        for (worker, state) in [
            (&child, &child_finished),
            (&grandchild, &grandchild_finished),
        ] {
            let mut event = protocol::Event::InternalWorker {
                worker: worker.clone(),
                event: Box::new(protocol::Event::WorkerState {
                    snapshot: state.clone(),
                }),
            };
            if worker == &grandchild {
                event = protocol::Event::InternalWorker {
                    worker: child.clone(),
                    event: Box::new(event),
                };
            }
            let _ = owner.publish_protocol_event(event).unwrap();
        }
        let cursor = runtime
            .worker_observation_cursor_now(&created.worker_ref)
            .unwrap();
        let published_children = |event: protocol::Event| {
            let _ = owner.publish_protocol_event(event).unwrap();
            let events = runtime
                .read_worker_observation_events(&created.worker_ref, cursor.clone())
                .unwrap();
            let protocol::Event::Snapshot {
                internal_workers, ..
            } = &events.last().unwrap().payload
            else {
                panic!("expected public full snapshot");
            };
            internal_workers.clone()
        };
        let result = published_children(parent_snapshot(vec![older.clone()]));
        assert_eq!(result[0].state, Some(child_finished.clone()));
        assert_eq!(result[0].status, protocol::WorkerStatus::Idle);
        assert_eq!(
            result[0].internal_workers[0].state,
            Some(grandchild_finished.clone())
        );
        let mut unavailable = older.clone();
        unavailable.state = None;
        // An unavailable ancestor closes descendants but must not erase a
        // higher nested high-water mark delivered in that same snapshot.
        let grandchild_newer = finished(10, "grandchild-next");
        unavailable.internal_workers[0].state = Some(grandchild_newer.clone());
        let result = published_children(parent_snapshot(vec![unavailable]));
        assert!(result[0].state.is_none());
        assert_eq!(
            result[0].internal_workers[0].state,
            Some(grandchild_newer.clone())
        );
        assert!(
            !runtime
                .lock()
                .unwrap()
                .worker(&created.worker_ref)
                .unwrap()
                .active_execution()
                .unwrap()
                .internal_workers
                .has_running_worker()
        );
        let result = published_children(parent_snapshot(vec![older.clone()]));
        assert!(result[0].state.is_none());
        assert!(result[0].internal_workers[0].state.is_none());
        let mut recovered = older.clone();
        recovered.state = Some(child_finished.clone());
        // The child's fresh observation cannot bypass its grandchild's own fence.
        let result = published_children(parent_snapshot(vec![recovered.clone()]));
        assert_eq!(result[0].state, Some(child_finished));
        assert!(result[0].internal_workers[0].state.is_none());
        recovered.internal_workers[0].state = Some(grandchild_finished);
        let result = published_children(parent_snapshot(vec![recovered.clone()]));
        assert!(result[0].internal_workers[0].state.is_none());
        recovered.internal_workers[0].state = Some(grandchild_newer.clone());
        let result = published_children(parent_snapshot(vec![recovered]));
        assert_eq!(result[0].internal_workers[0].state, Some(grandchild_newer));
        // A terminal membership snapshot removes the old session's high-water state.
        assert!(published_children(parent_snapshot(vec![])).is_empty());
        older.worker = internal_worker_ref("replacement", None);
        older.internal_workers.clear();
        let result = published_children(parent_snapshot(vec![older]));
        assert_eq!(result[0].state, Some(running(3)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn registry_overlap_publication_and_reconnect_agree_on_nested_completion() {
        use worker::spawn::registry::observation_fixture;
        // Production registry forwarders/reconnect reduction -> execution-owned
        // Runtime publication -> public cursor. Only source transport is injected.
        for service in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let (runtime, backend) = runtime_and_backend();
            let created = runtime
                .create_worker(task_request("registry overlap consumer"))
                .unwrap();
            let owner = context(&backend, &created.worker_ref);
            let cursor = runtime
                .worker_observation_cursor_now(&created.worker_ref)
                .unwrap();
            let reconnect =
                observation_fixture::exercise_snapshot_overlap(root.path(), service, |event| {
                    let _ = owner.publish_protocol_event(event).unwrap();
                })
                .await;
            let events = runtime
                .read_worker_observation_events(&created.worker_ref, cursor.clone())
                .unwrap();
            let protocol::Event::InternalWorker { event, .. } = &events.last().unwrap().payload
            else {
                panic!("expected forwarded source event");
            };
            let protocol::Event::Snapshot {
                internal_workers, ..
            } = &**event
            else {
                panic!("expected public child snapshot");
            };
            assert_eq!(
                serde_json::to_value(internal_workers).unwrap(),
                serde_json::to_value(&reconnect.internal_workers).unwrap()
            );
            let child = &internal_workers[0];
            let grandchild = &child.internal_workers[0];
            for (snapshot, sequence, marker) in
                [(child, 4, "child-run"), (grandchild, 31, "grandchild-run")]
            {
                let state = snapshot.state.as_ref().unwrap();
                assert_eq!(state.catalog_status(), protocol::WorkerStatus::Idle);
                assert_eq!(state.state_sequence, sequence);
                assert_eq!(
                    state.last_finished_submission_request_id.as_deref(),
                    Some(marker)
                );
            }
            let _ = owner
                .publish_protocol_event(parent_snapshot(vec![reconnect.clone()]))
                .unwrap();
            let events = runtime
                .read_worker_observation_events(&created.worker_ref, cursor)
                .unwrap();
            let protocol::Event::Snapshot {
                internal_workers, ..
            } = &events.last().unwrap().payload
            else {
                panic!("expected public reconnect snapshot");
            };
            assert_eq!(
                serde_json::to_value(internal_workers).unwrap(),
                serde_json::to_value(vec![reconnect]).unwrap()
            );
            assert!(
                !runtime
                    .lock()
                    .unwrap()
                    .worker(&created.worker_ref)
                    .unwrap()
                    .active_execution()
                    .unwrap()
                    .internal_workers
                    .has_running_worker()
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn internal_registry_loss_reaches_runtime_and_fences_queued_overlap() {
        use worker::spawn::registry::observation_fixture::{self, Loss};
        for service in [false, true] {
            for loss in [Loss::Lagged, Loss::Closed, Loss::Shutdown] {
                let root = tempfile::tempdir().unwrap();
                let (runtime, backend) = runtime_and_backend();
                let created = runtime
                    .create_worker(task_request("registry consumer"))
                    .unwrap();
                let owner = context(&backend, &created.worker_ref);
                let cursor = runtime
                    .worker_observation_cursor_now(&created.worker_ref)
                    .unwrap();
                let (lost, fresh) = observation_fixture::exercise(root.path(), service, loss, |event| {
                    let is_running = matches!(&event, protocol::Event::InternalWorker { event, .. }
                        if matches!(&**event, protocol::Event::WorkerState { snapshot } if snapshot.catalog_status() == protocol::WorkerStatus::Running));
                    let ordinary_error = matches!(&event, protocol::Event::InternalWorker { event, .. } if matches!(&**event, protocol::Event::Error { message, .. } if message == "ordinary diagnostic"));
                    let _ = owner.publish_protocol_event(event).unwrap();
                    if is_running || ordinary_error { assert!(runtime.lock().unwrap().worker(&created.worker_ref).unwrap().active_execution().unwrap().internal_workers.has_running_worker()); }
                }).await;
                assert!(
                    !runtime
                        .lock()
                        .unwrap()
                        .worker(&created.worker_ref)
                        .unwrap()
                        .active_execution()
                        .unwrap()
                        .internal_workers
                        .has_running_worker()
                );
                assert!(runtime.read_worker_observation_events(&created.worker_ref, cursor).unwrap().iter().any(|event|
                    matches!(event.payload, protocol::Event::InternalWorkerObservationUnavailable { .. })
                    || matches!(&event.payload, protocol::Event::InternalWorker { event, .. } if matches!(**event, protocol::Event::Shutdown))));
                let _ = owner
                    .publish_protocol_event(parent_snapshot(vec![lost.clone()]))
                    .unwrap();
                for sequence in [1, 2] {
                    let _ = owner
                        .publish_protocol_event(protocol::Event::InternalWorker {
                            worker: lost.worker.clone(),
                            event: Box::new(protocol::Event::WorkerState {
                                snapshot: running(sequence),
                            }),
                        })
                        .unwrap();
                }
                let _ = owner
                    .publish_protocol_event(protocol::Event::InternalWorker {
                        worker: lost.worker.clone(),
                        event: Box::new(protocol::Event::Snapshot {
                            session: lost.session.clone(),
                            greeting: lost.greeting.clone().unwrap(),
                            state: running(1),
                            in_flight: lost.in_flight.clone(),
                            internal_workers: vec![],
                        }),
                    })
                    .unwrap();
                assert!(
                    !runtime
                        .lock()
                        .unwrap()
                        .worker(&created.worker_ref)
                        .unwrap()
                        .active_execution()
                        .unwrap()
                        .internal_workers
                        .has_running_worker()
                );
                if let Some(fresh) = fresh {
                    assert!(fresh.state.is_some());
                    let _ = owner
                        .publish_protocol_event(parent_snapshot(vec![fresh]))
                        .unwrap();
                    assert!(
                        runtime
                            .lock()
                            .unwrap()
                            .worker(&created.worker_ref)
                            .unwrap()
                            .active_execution()
                            .unwrap()
                            .internal_workers
                            .has_running_worker()
                    );
                }
            }
        }
    }

    fn running(state_sequence: u64) -> protocol::WorkerStateSnapshot {
        protocol::WorkerStateSnapshot {
            state_sequence,
            state: protocol::WorkerState::Busy(protocol::WorkerBusyState::Run(
                protocol::WorkerRunState::Running,
            )),
            ..protocol::WorkerStateSnapshot::initial()
        }
    }

    fn context(backend: &TestExecutionBackend, worker: &WorkerRef) -> WorkerExecutionContext {
        backend.contexts.lock().unwrap()[&worker.worker_id].clone()
    }

    fn snapshot(runtime: &Runtime, worker: &WorkerRef) -> Option<protocol::WorkerStateSnapshot> {
        runtime
            .lock()
            .unwrap()
            .worker(worker)
            .unwrap()
            .worker_state()
    }

    #[test]
    fn observation_create_commit_keeps_live_tail_ahead_of_retained_initial_and_response() {
        // Runtime create/commit and publication are production implementations;
        // the backend controls the exact snapshot/live overlap without a scheduler race.
        for initial_input in [false, true] {
            let (runtime, backend) = runtime_and_backend();
            let latest = running(2);
            *backend.create_event.lock().unwrap() = Some(protocol::Event::WorkerState {
                snapshot: latest.clone(),
            });
            let mut request = task_request("create overlap");
            if initial_input {
                request.initial_input = Some(WorkerInput::user("begin"));
                backend.set_dispatch_result(
                    WorkerExecutionResult::accepted_submission(
                        WorkerExecutionOperation::Input,
                        "assigned-by-runtime",
                        "submission",
                        protocol::SubmissionDisposition::Started,
                    )
                    .with_worker_state(protocol::WorkerStateSnapshot::initial()),
                );
            }
            let created = runtime.create_worker(request).unwrap();
            assert_eq!(created.worker_state, Some(latest.clone()));
            assert_eq!(snapshot(&runtime, &created.worker_ref), Some(latest));
            assert_eq!(
                created.observation,
                WorkerObservationState::Current { state_sequence: 2 }
            );
        }
    }

    #[test]
    fn observation_restore_flush_and_late_response_cannot_undo_finished_submission() {
        let (runtime, backend) = runtime_and_backend();
        let created = runtime
            .create_worker(task_request("restore overlap"))
            .unwrap();
        let old_context = context(&backend, &created.worker_ref);
        runtime.stop_worker(&created.worker_ref, None).unwrap();
        let finished = protocol::WorkerStateSnapshot {
            state_sequence: 4,
            last_finished_submission_request_id: Some("finished-current".into()),
            ..protocol::WorkerStateSnapshot::initial()
        };
        *backend.restore_event.lock().unwrap() = Some(protocol::Event::WorkerState {
            snapshot: finished.clone(),
        });
        let restored = runtime
            .restore_worker(
                &created.worker_ref,
                runtime.test_restore_request(&created.worker_ref),
            )
            .unwrap();
        assert_eq!(restored.worker_state, Some(finished.clone()));
        assert!(
            old_context
                .publish_protocol_event(protocol::Event::WorkerState {
                    snapshot: running(999),
                })
                .is_err()
        );
        assert_eq!(
            snapshot(&runtime, &created.worker_ref),
            Some(finished.clone())
        );
        runtime
            .lock()
            .unwrap()
            .commit_response_snapshot(
                &created.worker_ref,
                &protocol::WorkerStateSnapshot::initial(),
            )
            .unwrap();
        assert_eq!(snapshot(&runtime, &created.worker_ref), Some(finished));
        assert_eq!(
            runtime
                .lock()
                .unwrap()
                .worker(&created.worker_ref)
                .unwrap()
                .last_finished_submission_request_id
                .as_deref(),
            Some("finished-current")
        );
    }

    #[test]
    fn observation_inactive_and_replaced_owner_cannot_publish_even_nonstate_events() {
        let (runtime, backend) = runtime_and_backend();
        let created = runtime
            .create_worker(task_request("owner lifetime"))
            .unwrap();
        let old = context(&backend, &created.worker_ref);
        runtime.stop_worker(&created.worker_ref, None).unwrap();
        assert!(
            old.publish_protocol_event(protocol::Event::TextDelta {
                text: "late".into()
            })
            .is_err()
        );
        runtime
            .restore_worker(
                &created.worker_ref,
                runtime.test_restore_request(&created.worker_ref),
            )
            .unwrap();
        assert!(
            old.publish_protocol_event(protocol::Event::TextDelta {
                text: "old execution".into()
            })
            .is_err()
        );
        assert!(matches!(
            context(&backend, &created.worker_ref)
                .publish_protocol_event(protocol::Event::TextDelta {
                    text: "current".into()
                })
                .unwrap(),
            WorkerObservationPublication::Published(_)
        ));
    }

    #[cfg(feature = "fs-store")]
    #[test]
    fn observation_commit_failure_invalidates_then_same_owner_recovers_durable_facts() {
        for fault in [
            crate::fs_store::AtomicWriteFault::BeforeRename,
            crate::fs_store::AtomicWriteFault::AfterRename,
        ] {
            let root = tempfile::tempdir().unwrap();
            let backend = Arc::new(TestExecutionBackend::default());
            let runtime = Runtime::with_fs_store_and_execution_backend(
                FsRuntimeStoreOptions::new(root.path()).with_runtime_id("observation-fault"),
                backend.clone(),
            )
            .unwrap();
            runtime.store_config_bundle(test_bundle()).unwrap();
            let created = runtime
                .create_worker(task_request("durable observation"))
                .unwrap();
            let owner = context(&backend, &created.worker_ref);
            let finished = protocol::WorkerStateSnapshot {
                state_sequence: 2,
                last_finished_submission_request_id: Some("finished-durable".into()),
                ..protocol::WorkerStateSnapshot::initial()
            };
            let cursor = runtime
                .worker_observation_cursor_now(&created.worker_ref)
                .unwrap();
            runtime_store(&runtime).fail_next_worker_write(fault);
            assert!(
                owner
                    .publish_protocol_event(protocol::Event::WorkerState {
                        snapshot: finished.clone()
                    })
                    .is_err()
            );
            assert!(snapshot(&runtime, &created.worker_ref).is_none());
            assert!(runtime.read_worker_observation_events(&created.worker_ref, cursor).unwrap().iter().all(|event|
                !matches!(&event.payload, protocol::Event::WorkerState { snapshot }
                    if snapshot.last_finished_submission_request_id.as_deref() == Some("finished-durable"))));
            let uncertain = runtime
                .lock()
                .unwrap()
                .unreconciled_worker_writes
                .contains(&created.worker_id);
            assert_eq!(
                uncertain,
                fault == crate::fs_store::AtomicWriteFault::AfterRename
            );
            owner
                .publish_protocol_event(protocol::Event::WorkerState {
                    snapshot: finished.clone(),
                })
                .unwrap();
            assert_eq!(snapshot(&runtime, &created.worker_ref), Some(finished));
            assert!(
                !runtime
                    .lock()
                    .unwrap()
                    .unreconciled_worker_writes
                    .contains(&created.worker_id)
            );
            assert_eq!(
                runtime_store(&runtime)
                    .read_durable_worker_record(&created.worker_ref)
                    .unwrap()
                    .last_finished_submission_request_id
                    .as_deref(),
                Some("finished-durable")
            );
        }
    }

    #[cfg(feature = "fs-store")]
    #[test]
    fn observation_duplicate_snapshot_does_not_write_or_rotate_token() {
        let root = tempfile::tempdir().unwrap();
        let backend = Arc::new(TestExecutionBackend::default());
        let runtime = Runtime::with_fs_store_and_execution_backend(
            FsRuntimeStoreOptions::new(root.path()).with_runtime_id("observation-duplicate"),
            backend.clone(),
        )
        .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let created = runtime.create_worker(task_request("duplicate")).unwrap();
        let owner = context(&backend, &created.worker_ref);
        let latest = running(1);
        owner
            .publish_protocol_event(protocol::Event::WorkerState {
                snapshot: latest.clone(),
            })
            .unwrap();
        let before = runtime
            .lock()
            .unwrap()
            .worker(&created.worker_ref)
            .unwrap()
            .restore_guard
            .observation_token
            .clone();
        runtime_store(&runtime)
            .fail_next_worker_write(crate::fs_store::AtomicWriteFault::BeforeRename);
        owner
            .publish_protocol_event(protocol::Event::WorkerState { snapshot: latest })
            .unwrap();
        let after = runtime
            .lock()
            .unwrap()
            .worker(&created.worker_ref)
            .unwrap()
            .restore_guard
            .observation_token
            .clone();
        assert_eq!(before, after);
        // The fault is still armed: a new durable fact must encounter it.
        assert!(
            owner
                .publish_protocol_event(protocol::Event::WorkerState {
                    snapshot: protocol::WorkerStateSnapshot {
                        state_sequence: 2,
                        ..protocol::WorkerStateSnapshot::initial()
                    },
                })
                .is_err()
        );
    }

    #[test]
    fn observation_publication_failure_invalidates_without_losing_owner_recovery() {
        let (runtime, backend) = runtime_and_backend();
        let created = runtime
            .create_worker(task_request("publication failure"))
            .unwrap();
        let owner = context(&backend, &created.worker_ref);
        runtime.lock().unwrap().fail_next_observation_publication = true;
        assert!(
            owner
                .publish_protocol_event(protocol::Event::WorkerState {
                    snapshot: running(1)
                })
                .is_err()
        );
        assert!(snapshot(&runtime, &created.worker_ref).is_none());
        owner
            .publish_protocol_event(protocol::Event::WorkerState {
                snapshot: running(1),
            })
            .unwrap();
        assert_eq!(snapshot(&runtime, &created.worker_ref), Some(running(1)));
    }

    #[test]
    fn observation_fresh_detail_uses_execution_snapshot_not_catalog_idle() {
        let (runtime, backend) = runtime_and_backend();
        let created = runtime.create_worker(task_request("fresh detail")).unwrap();
        backend.set_worker_snapshot(
            &created.worker_ref,
            protocol::Event::WorkerState {
                snapshot: running(1),
            },
        );
        let actual = runtime.worker_detail(&created.worker_ref).unwrap();
        assert_eq!(actual.status, WorkerStatus::Running);
        assert_eq!(
            actual.observation,
            WorkerObservationState::Current { state_sequence: 1 }
        );
        assert_eq!(actual.worker_state, Some(running(1)));
        backend.snapshots.lock().unwrap().remove(&created.worker_id);
        let unavailable = runtime.worker_detail(&created.worker_ref).unwrap();
        assert_eq!(unavailable.observation, WorkerObservationState::Unavailable);
        assert!(unavailable.worker_state.is_none());
    }
}
