// Included by the production Profile publication suite. Each fault is at a
// distinct partial-publication boundary; no factory result is substituted.

fn publication_gate<T>() -> (
    mpsc::Sender<()>,
    mpsc::Receiver<T>,
    crate::execution::RestoreTestGate<T>,
) {
    let (release, receiver) = mpsc::channel();
    let (entered, evidence) = mpsc::channel();
    (
        release,
        evidence,
        crate::execution::RestoreTestGate {
            entered,
            release: receiver,
        },
    )
}

fn assert_single_publication(fixture: &Fixture, expected: usize) {
    let faults = &fixture.backend.factory.restore_faults;
    assert_eq!(
        faults.executions.load(Ordering::SeqCst),
        expected,
        "construction reran"
    );
    assert_eq!(
        faults.effects_calls.load(Ordering::SeqCst),
        expected,
        "restore effects reran"
    );
    assert_eq!(
        faults.publication_attempts.load(Ordering::SeqCst),
        expected,
        "Controller start reran"
    );
    fixture.assert_quiet();
}

fn assert_real_attach(
    fixture: &Fixture,
    handle: &WorkerHandle,
) -> crate::execution::WorkerProtocolTransport {
    let worker_ref = &fixture.request.worker_ref;
    assert!(matches!(
        fixture
            .backend
            .worker_session(crate::execution::WorkerSessionObservationRequest {
                worker_ref: worker_ref.clone(),
            }),
        runtime_api::WorkerSessionAvailability::LiveProtocol
    ));
    let transport = fixture
        .backend
        .clone()
        .attach_worker_protocol(worker_ref)
        .unwrap();
    transport.validate().unwrap();
    let paired = subscribe_worker_protocol_session(handle);
    assert_eq!(
        serde_json::to_value(&transport.snapshot).unwrap(),
        serde_json::to_value(paired.snapshot_event).unwrap()
    );
    assert!(
        matches!(&transport.snapshot, Event::Snapshot { session, greeting, state, in_flight, .. }
        if session == &session_store::public_snapshot::project_current_session_snapshot(&fixture.entries())
        && !greeting.tools.is_empty() && state.state == protocol::WorkerState::Idle && in_flight.is_empty())
    );
    transport
}

fn observe_publication_markers(
    fixture: &Fixture,
    observations: tokio::sync::broadcast::Receiver<crate::observation::WorkerObservationEvent>,
    markers: &'static [&'static str],
) -> tokio::sync::broadcast::Receiver<crate::observation::WorkerObservationEvent> {
    fixture
        .backend
        .run_on_adapter_runtime(async move {
            let mut observations = observations;
            let mut counts = vec![0; markers.len()];
            let mut snapshots = 0;
            tokio::time::timeout(WAIT, async {
                loop {
                    let observed = observations.recv().await.unwrap();
                    if matches!(observed.payload, Event::Snapshot { .. }) {
                        snapshots += 1;
                    }
                    for (index, marker) in markers.iter().enumerate() {
                        if is_marker(&observed.payload, marker) {
                            counts[index] += 1;
                        }
                    }
                    if counts.last() == Some(&1) {
                        break;
                    }
                }
            })
            .await
            .map_err(|_| format!("lost original subscription markers: {counts:?}"))?;
            assert_eq!(
                counts,
                vec![1; markers.len()],
                "lost or duplicated prefix on original receiver"
            );
            assert_eq!(snapshots, 1, "initial full snapshot was lost or replayed");
            while let Ok(event) = observations.try_recv() {
                assert!(
                    !markers
                        .iter()
                        .any(|marker| is_marker(&event.payload, marker)),
                    "duplicate after tail fence"
                );
            }
            Ok(observations)
        })
        .unwrap()
}

fn partial_registration_recovers_same_controller(hub: bool) {
    let mut fixture = Fixture::no_model();
    Arc::get_mut(&mut fixture.backend)
        .unwrap()
        .spawn_restore_timeout = Duration::from_millis(250);
    let runtime = stopped_runtime(&fixture);
    let pause = runtime.test_pause_restore_continuation();
    let worker_ref = fixture.request.worker_ref.clone();
    let scope = fixture.request.workspace_scope.as_ref().unwrap();
    let request = runtime.test_restore_request(&worker_ref);
    let observations = runtime.subscribe_worker_observation().unwrap();
    let (started_release, started_entered) = activation_gate(&fixture, true);
    let first = runtime
        .restore_worker_operation_scoped(scope, &worker_ref, request.clone())
        .unwrap();
    assert_eq!(
        first.state,
        server_api::WorkerRestoreState::ReconciliationRequired
    );
    let started = started_entered.recv_timeout(WAIT).unwrap();
    let pending = assert_activation_owned(&fixture, &runtime, &request.request_id, &started);
    let old = started.handle.as_ref().unwrap().clone();
    let canonical = fixture.canonical_files();
    alert_marker(&old, "registration-original-paired-prefix");
    let (release, entered, fault) = publication_gate();
    if hub {
        *fixture
            .backend
            .factory
            .observation_hub
            .registration_gate
            .lock()
            .unwrap() = Some(RestoreHubRegistrationGate {
            gate: fault,
            fail: true,
        });
    } else {
        *fixture.backend.bridge_fault.lock().unwrap() = Some(fault);
    }
    started_release.release();
    drop(pause);
    let boundary_handle = entered.recv_timeout(WAIT).unwrap();
    assert!(boundary_handle.same_controller(&old));
    let durable = durable_operation(&runtime, &worker_ref).unwrap();
    assert_eq!(durable.operation_id, started.operation_id);
    assert_eq!(durable.commit, crate::runtime::RestoreCommitFact::Committed);
    assert!(restore_owner(&runtime, &worker_ref, &request.request_id)["receipt"].is_null());
    if hub {
        let (_, shared, _) = fixture
            .backend
            .factory
            .observation_hub
            .get(&worker_ref)
            .unwrap();
        assert!(
            Arc::ptr_eq(&shared, &old.shared_state),
            "real hub insert preceded fault"
        );
    } else {
        assert!(
            fixture
                .backend
                .factory
                .observation_hub
                .get(&worker_ref)
                .is_none()
        );
    }
    let (operation, context, uncertain) = runtime.test_pending_restore_context(&worker_ref);
    assert_eq!(operation, started.operation_id);
    assert!(!uncertain);
    assert!(
        context
            .test_buffered_events()
            .iter()
            .any(|event| matches!(event, Event::Snapshot { .. }))
    );
    alert_marker(&old, "registration-during-fault");
    let pause = runtime.test_pause_restore_continuation();
    release.send(()).unwrap();
    // This lock also observes the end of the actual registration attempt, not
    // merely the test's entered notification. Its retained scope owns cleanup.
    let execution = fixture.backend.workers.lock().unwrap()[&worker_ref].clone();
    assert!(old.same_controller(&execution.handle));
    assert_eq!(execution.restore_operation_id, Some(operation));
    assert_eq!(
        execution.tasks.tasks.lock().unwrap().len(),
        if hub { 2 } else { 1 }
    );
    let bridge = execution.tasks.protocol_bridge.lock().unwrap();
    if hub {
        assert!(bridge.is_none());
    } else {
        let bridge = bridge.as_ref().unwrap();
        assert!(
            !bridge.snapshot_pending,
            "snapshot already registered, do not replay it"
        );
        let streams = bridge.streams.lock().unwrap();
        assert!(
            streams.is_some(),
            "original paired streams survived failed task admission"
        );
        assert_eq!(
            serde_json::to_value(&streams.as_ref().unwrap().snapshot_event).unwrap(),
            serde_json::to_value(&started.snapshot).unwrap()
        );
    }
    drop(bridge);
    assert!(
        !old.protocol_is_closed(),
        "publication failure must not clean a committed Controller"
    );
    assert_single_publication(&fixture, 1);
    drop(pause);
    runtime.test_wait_restore_continuation(&worker_ref, operation, WAIT);
    assert!(durable_operation(&runtime, &worker_ref).is_none());
    let owner = restore_owner(&runtime, &worker_ref, &request.request_id);
    assert_eq!(
        owner["operation_id"],
        serde_json::to_value(operation).unwrap()
    );
    assert_eq!(owner["receipt"]["state"], "accepted");
    assert!(
        fixture
            .backend
            .pending_restore(&worker_ref)
            .unwrap()
            .is_none()
    );
    assert!(!pending.task_in_flight.load(Ordering::Acquire));
    assert!(pending.activated.lock().unwrap().is_none());
    assert!(pending.result.lock().unwrap().is_none());
    assert!(pending.task.try_lock().unwrap().is_none());
    assert_eq!(fixture.backend.workers.lock().unwrap().len(), 1);
    assert!(old.same_controller(&fixture.backend.workers.lock().unwrap()[&worker_ref].handle));
    let transport = assert_real_attach(&fixture, &old);
    assert_eq!(fixture.canonical_files(), canonical);
    assert_single_publication(&fixture, 1);
    alert_marker(&old, "registration-tail-fence");
    let observations = observe_publication_markers(
        &fixture,
        observations,
        &[
            "registration-original-paired-prefix",
            "registration-during-fault",
            "registration-tail-fence",
        ],
    );
    assert_eq!(
        runtime
            .restore_worker_operation_scoped(scope, &worker_ref, request.clone())
            .unwrap()
            .state,
        server_api::WorkerRestoreState::Accepted
    );
    assert_single_publication(&fixture, 1);
    // Retain the original transport/handle and scope through Stop/replacement.
    runtime
        .stop_worker_scoped(scope, &worker_ref, None)
        .unwrap();
    assert!(old.protocol_is_closed());
    assert!(transport.validate().is_err());
    let replacement_request = runtime.test_restore_request(&worker_ref);
    assert_eq!(
        runtime
            .restore_worker_operation_scoped(scope, &worker_ref, replacement_request.clone())
            .unwrap()
            .state,
        server_api::WorkerRestoreState::Accepted
    );
    let replacement = fixture.backend.workers.lock().unwrap()[&worker_ref].clone();
    assert!(!old.same_controller(&replacement.handle));
    assert_ne!(replacement.restore_operation_id, Some(operation));
    assert!(
        transport
            .dispatch(Method::Pause {
                command: protocol::WorkerCommandEnvelope::new(801)
            })
            .is_err()
    );
    assert!(
        transport
            .dispatch(Method::Shutdown {
                command: protocol::WorkerCommandEnvelope::new(802)
            })
            .is_err()
    );
    for method in [
        Method::Pause {
            command: protocol::WorkerCommandEnvelope::new(803),
        },
        Method::Shutdown {
            command: protocol::WorkerCommandEnvelope::new(804),
        },
    ] {
        let old_command = old.clone();
        assert!(
            fixture
                .backend
                .run_on_adapter_runtime(async move {
                    old_command
                        .send(method)
                        .await
                        .map_err(|error| error.to_string())
                })
                .is_err(),
            "closed original Controller command must not route to replacement"
        );
    }
    fixture
        .backend
        .factory
        .deactivate_stopped_controller(&worker_ref, &old)
        .unwrap();
    let (_, shared, _) = fixture
        .backend
        .factory
        .observation_hub
        .get(&worker_ref)
        .unwrap();
    assert!(Arc::ptr_eq(&shared, &replacement.handle.shared_state));
    alert_marker(&old, "registration-stopped-event-must-not-leak");
    alert_marker(&replacement.handle, "registration-replacement-fence");
    fixture
        .backend
        .run_on_adapter_runtime(async move {
            let mut observations = observations;
            tokio::time::timeout(WAIT, async {
                loop {
                    let event = observations.recv().await.unwrap();
                    assert!(!is_marker(
                        &event.payload,
                        "registration-stopped-event-must-not-leak"
                    ));
                    if is_marker(&event.payload, "registration-replacement-fence") {
                        break;
                    }
                }
            })
            .await
            .map_err(|_| "replacement observation fence missing".to_string())?;
            drop(transport);
            Ok(())
        })
        .unwrap();
    assert_single_publication(&fixture, 2);
    assert_eq!(
        runtime
            .restore_worker_operation_scoped(scope, &worker_ref, request.clone())
            .unwrap()
            .state,
        server_api::WorkerRestoreState::Accepted
    );
    assert_eq!(restore_owner_count(&runtime, &worker_ref), 2);
    let current = fixture.backend.workers.lock().unwrap()[&worker_ref].clone();
    assert!(current.handle.same_controller(&replacement.handle));
    assert_eq!(
        current.restore_operation_id,
        replacement.restore_operation_id
    );
    assert_eq!(
        replacement.handle.shared_state.snapshot().state,
        protocol::WorkerState::Idle
    );
    drop(current);
    drop(execution);
    drop(replacement);
    drop(pending);
    runtime
        .stop_worker_scoped(scope, &worker_ref, None)
        .unwrap();
    fixture.assert_released();
}

fn restore_owner_count(runtime: &EmbeddedRuntime, worker_ref: &WorkerRef) -> usize {
    let saved = runtime
        .test_runtime_store()
        .read_worker_record(worker_ref)
        .unwrap();
    serde_json::to_value(saved.restore_guard).unwrap()["owners"]
        .as_object()
        .unwrap()
        .len()
}

#[test]
#[serial_test::serial(worker_allocation)]
fn bridge_partial_snapshot_registration_retains_original_streams_and_autonomously_recovers() {
    partial_registration_recovers_same_controller(false);
}

#[test]
#[serial_test::serial(worker_allocation)]
fn hub_partial_insert_registration_retains_original_controller_and_autonomously_recovers() {
    partial_registration_recovers_same_controller(true);
}

#[test]
#[serial_test::serial(worker_allocation)]
fn context_partial_flush_retains_only_undelivered_queue_and_recovers_without_repost() {
    let mut fixture = Fixture::no_model();
    Arc::get_mut(&mut fixture.backend)
        .unwrap()
        .spawn_restore_timeout = Duration::from_millis(250);
    let runtime = stopped_runtime(&fixture);
    let pause = runtime.test_pause_restore_continuation();
    let worker_ref = fixture.request.worker_ref.clone();
    let scope = fixture.request.workspace_scope.as_ref().unwrap();
    let request = runtime.test_restore_request(&worker_ref);
    let observations = runtime.subscribe_worker_observation().unwrap();
    let (started_release, started_entered) = activation_gate(&fixture, true);
    assert_eq!(
        runtime
            .restore_worker_operation_scoped(scope, &worker_ref, request.clone())
            .unwrap()
            .state,
        server_api::WorkerRestoreState::ReconciliationRequired
    );
    let started = started_entered.recv_timeout(WAIT).unwrap();
    let pending = assert_activation_owned(&fixture, &runtime, &request.request_id, &started);
    let old = started.handle.unwrap();
    let (operation, context, _) = runtime.test_pending_restore_context(&worker_ref);
    // Stop after the actual bridge and hub registration, without failing that
    // boundary, so the flush begins with its real original full snapshot.
    let (hub_release, hub_entered, hub_gate) = publication_gate();
    *fixture
        .backend
        .factory
        .observation_hub
        .registration_gate
        .lock()
        .unwrap() = Some(RestoreHubRegistrationGate {
        gate: hub_gate,
        fail: false,
    });
    started_release.release();
    drop(pause);
    assert!(
        hub_entered
            .recv_timeout(WAIT)
            .unwrap()
            .same_controller(&old)
    );
    let buffered_prefix = context.test_buffered_events();
    assert!(matches!(
        buffered_prefix.first(),
        Some(Event::Snapshot { .. })
    ));
    let prefix_length = buffered_prefix.len();
    let marker = |text: &str| Event::Error {
        code: protocol::ErrorCode::Internal,
        message: text.into(),
    };
    context
        .publish_protocol_event(marker("flush-delivered"))
        .unwrap();
    context
        .publish_protocol_event(marker("flush-failed"))
        .unwrap();
    context
        .publish_protocol_event(marker("flush-pending"))
        .unwrap();
    let (release, entered, gate) = publication_gate();
    let (restored, restored_queue) = mpsc::channel();
    context.test_fail_flush(crate::execution::RestoreFlushFault {
        remaining_successes: prefix_length + 1,
        gate,
        restored,
    });
    drop(hub_release);
    assert!(
        matches!(entered.recv_timeout(WAIT).unwrap(), Event::Error { message, .. } if message == "flush-failed")
    );
    let partial_execution = fixture.backend.workers.lock().unwrap()[&worker_ref].clone();
    assert!(old.same_controller(&partial_execution.handle));
    assert_eq!(partial_execution.restore_operation_id, Some(operation));
    assert_eq!(partial_execution.tasks.tasks.lock().unwrap().len(), 2);
    let (_, shared, _) = fixture
        .backend
        .factory
        .observation_hub
        .get(&worker_ref)
        .unwrap();
    assert!(Arc::ptr_eq(&shared, &old.shared_state));
    let pause = runtime.test_pause_restore_continuation();
    context
        .publish_protocol_event(marker("flush-arrived-during-error"))
        .unwrap();
    release.send(()).unwrap();
    let restored = restored_queue.recv_timeout(WAIT).unwrap();
    let undelivered: Vec<_> = restored
        .iter()
        .filter_map(|event| match event {
            Event::Error { message, .. } if message.starts_with("flush-") => Some(message.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        undelivered,
        [
            "flush-failed",
            "flush-pending",
            "flush-arrived-during-error"
        ]
    );
    assert!(
        !restored
            .iter()
            .any(|event| matches!(event, Event::Snapshot { .. })),
        "delivered snapshot must not rejoin the failed suffix"
    );
    assert_eq!(
        serde_json::to_value(context.test_buffered_events()).unwrap(),
        serde_json::to_value(&restored).unwrap()
    );
    assert_eq!(
        durable_operation(&runtime, &worker_ref)
            .unwrap()
            .operation_id,
        operation
    );
    assert!(restore_owner(&runtime, &worker_ref, &request.request_id)["receipt"].is_null());
    // The next terminal gate observes successful retry only after the same
    // queued owner has repaired its failed callback and remaining suffix.
    let (terminal_release, terminal_entered, gate) = publication_gate();
    runtime.test_terminal_receipt_gate(gate);
    drop(pause);
    assert_eq!(terminal_entered.recv_timeout(WAIT).unwrap(), operation);
    assert!(context.test_buffered_events().is_empty());
    assert!(restore_owner(&runtime, &worker_ref, &request.request_id)["receipt"].is_null());
    assert_eq!(
        durable_operation(&runtime, &worker_ref).unwrap().commit,
        crate::runtime::RestoreCommitFact::Committed
    );
    let execution = fixture.backend.workers.lock().unwrap()[&worker_ref].clone();
    assert!(old.same_controller(&execution.handle));
    assert_eq!(execution.restore_operation_id, Some(operation));
    drop(terminal_release); // RAII disconnect, not a second request.
    runtime.test_wait_restore_continuation(&worker_ref, operation, WAIT);
    assert_eq!(
        restore_owner(&runtime, &worker_ref, &request.request_id)["receipt"]["state"],
        "accepted"
    );
    assert_real_attach(&fixture, &old);
    fixture
        .backend
        .run_on_adapter_runtime(async move {
            let mut observations = observations;
            let mut messages = Vec::new();
            let mut snapshots = 0;
            while let Ok(event) = observations.try_recv() {
                match event.payload {
                    Event::Error { message, .. } if message.starts_with("flush-") => {
                        messages.push(message)
                    }
                    Event::Snapshot { .. } => snapshots += 1,
                    _ => {}
                }
            }
            assert_eq!(
                messages,
                [
                    "flush-delivered",
                    "flush-failed",
                    "flush-pending",
                    "flush-arrived-during-error"
                ]
            );
            assert_eq!(snapshots, 1);
            Ok(())
        })
        .unwrap();
    assert_single_publication(&fixture, 1);
    drop(partial_execution);
    drop(execution);
    drop(pending);
    runtime
        .stop_worker_scoped(scope, &worker_ref, None)
        .unwrap();
    fixture.assert_released();
}

#[derive(Clone, Copy, Debug)]
enum TerminalFault {
    BeforeRename,
    AfterRename,
    SyncUnknown,
    ReadUnknown,
}

fn published_terminal_receipt_fault(fault: TerminalFault) {
    let mut fixture = Fixture::no_model();
    // Reuse the existing ordinary non-network model boundary, not the product
    // Codex profile. All fault/convergence assertions precede the sole input.
    let script = MockClient::sequential(vec![MockResponse::Complete(simple_text_events())]);
    let injected = script.clone();
    let factory = fixture
        .backend
        .factory
        .as_ref()
        .clone()
        .with_spawn_model_client_factory(move || Box::new(injected.clone()));
    fixture.backend = Arc::new(WorkerRuntimeExecutionBackend::new(factory).unwrap());
    let runtime = Arc::new(stopped_runtime(&fixture));
    let pause = runtime.test_pause_restore_continuation();
    let worker_ref = fixture.request.worker_ref.clone();
    let scope = fixture.request.workspace_scope.clone().unwrap();
    let request = runtime.test_restore_request(&worker_ref);
    let observations = runtime.subscribe_worker_observation().unwrap();
    let (release, entered, gate) = publication_gate();
    runtime.test_terminal_receipt_gate(gate);
    let restoring = {
        let runtime = runtime.clone();
        let worker_ref = worker_ref.clone();
        let scope = scope.clone();
        let request = request.clone();
        std::thread::spawn(move || {
            runtime.restore_worker_operation_scoped(&scope, &worker_ref, request)
        })
    };
    let operation = entered.recv_timeout(WAIT).unwrap();
    // Restore legitimately repairs derived metadata before publication. Receipt
    // failures must not rerun those effects; compare from this actual boundary.
    let canonical = fixture.canonical_files();
    let execution = fixture.backend.workers.lock().unwrap()[&worker_ref].clone();
    let old = execution.handle.clone();
    assert_eq!(execution.restore_operation_id, Some(operation));
    assert!(
        fixture
            .backend
            .pending_restore(&worker_ref)
            .unwrap()
            .is_none()
    );
    assert_eq!(execution.tasks.tasks.lock().unwrap().len(), 2);
    assert!(execution.tasks.protocol_bridge.lock().unwrap().is_none());
    assert_eq!(
        durable_operation(&runtime, &worker_ref)
            .unwrap()
            .operation_id,
        operation
    );
    assert_eq!(
        durable_operation(&runtime, &worker_ref).unwrap().commit,
        crate::runtime::RestoreCommitFact::Committed
    );
    let owner = restore_owner(&runtime, &worker_ref, &request.request_id);
    assert!(owner["receipt"].is_null());
    let (same_operation, context, uncertain) = runtime.test_pending_restore_context(&worker_ref);
    assert_eq!(same_operation, operation);
    assert!(!uncertain);
    assert!(context.test_buffered_events().is_empty());
    let transport = assert_real_attach(&fixture, &old);
    assert_single_publication(&fixture, 1);
    alert_marker(&old, "terminal-before-receipt");
    let store = runtime.test_runtime_store();
    let reconcile_before = store.test_worker_reconcile_counts();
    store.fail_next_worker_write(if matches!(fault, TerminalFault::BeforeRename) {
        AtomicWriteFault::BeforeRename
    } else {
        AtomicWriteFault::AfterRename
    });
    match fault {
        TerminalFault::SyncUnknown => store.fail_next_worker_reconcile_sync(),
        TerminalFault::ReadUnknown => store.fail_next_worker_reconcile_read(),
        _ => {}
    }
    release.send(()).unwrap();
    let first = restoring.join().unwrap().unwrap();
    let reconcile_after = store.test_worker_reconcile_counts();
    let reconciliation = (
        reconcile_after.0 - reconcile_before.0,
        reconcile_after.1 - reconcile_before.1,
    );
    assert_eq!(
        reconciliation,
        match fault {
            TerminalFault::BeforeRename => (0, 0),
            TerminalFault::AfterRename => (1, 4), // canonical file plus Worker/workers/root directories
            TerminalFault::SyncUnknown => (1, 1),
            TerminalFault::ReadUnknown => (1, 0),
        },
        "{fault:?}: actual canonical read/sync attempts"
    );
    assert_eq!(
        first.state,
        if matches!(fault, TerminalFault::AfterRename) {
            server_api::WorkerRestoreState::Accepted
        } else {
            server_api::WorkerRestoreState::ReconciliationRequired
        },
        "{fault:?}: {first:?}"
    );
    assert_eq!(fixture.canonical_files(), canonical);
    assert!(!old.protocol_is_closed());
    transport.validate().unwrap();
    assert_single_publication(&fixture, 1);
    let disk_owner = restore_owner(&runtime, &worker_ref, &request.request_id);
    assert_eq!(disk_owner["operation_id"], owner["operation_id"]);
    match fault {
        TerminalFault::BeforeRename => {
            assert!(disk_owner["receipt"].is_null());
            assert!(!runtime.test_pending_restore_context(&worker_ref).2);
        }
        TerminalFault::AfterRename => {
            assert_eq!(disk_owner["receipt"]["state"], "accepted");
            assert!(durable_operation(&runtime, &worker_ref).is_none());
        }
        TerminalFault::SyncUnknown | TerminalFault::ReadUnknown => {
            assert_eq!(
                disk_owner["receipt"]["state"], "accepted",
                "visible rename alone is not durable proof"
            );
            let (same_operation, _, fenced) = runtime.test_pending_restore_context(&worker_ref);
            assert_eq!(same_operation, operation);
            assert!(
                fenced,
                "unknown canonical outcome must retain the memory owner/write fence"
            );
            // Keep durability unproved for one competing Stop as well: it must
            // fail before any Shutdown, never manufacture a Stopped judgment.
            match fault {
                TerminalFault::SyncUnknown => store.fail_next_worker_reconcile_sync(),
                TerminalFault::ReadUnknown => store.fail_next_worker_reconcile_read(),
                _ => unreachable!(),
            }
            assert!(matches!(
                runtime.stop_worker_scoped(&scope, &worker_ref, None),
                Err(crate::error::RuntimeError::StoreCommitOutcomeUnknown { .. })
            ));
            assert!(!old.protocol_is_closed());
            assert!(!execution.shutdown_requested.load(Ordering::Acquire));
            assert!(runtime.test_pending_restore_context(&worker_ref).2);
            assert_single_publication(&fixture, 1);
        }
    }
    // Recovery is autonomous after the failed write / confirmed unknown fence.
    // No POST, fresh request, or input is needed to make it converge.
    drop(pause);
    if !matches!(fault, TerminalFault::AfterRename) {
        runtime.test_wait_restore_continuation(&worker_ref, operation, WAIT);
    }
    assert!(durable_operation(&runtime, &worker_ref).is_none());
    assert_eq!(
        restore_owner(&runtime, &worker_ref, &request.request_id)["receipt"]["state"],
        "accepted"
    );
    let current = fixture.backend.workers.lock().unwrap()[&worker_ref].clone();
    assert!(old.same_controller(&current.handle));
    assert_eq!(current.restore_operation_id, Some(operation));
    assert_eq!(fixture.backend.workers.lock().unwrap().len(), 1);
    assert_real_attach(&fixture, &old);
    assert_single_publication(&fixture, 1);
    alert_marker(&old, "terminal-after-recovery");
    let _observations = observe_publication_markers(
        &fixture,
        observations,
        &["terminal-before-receipt", "terminal-after-recovery"],
    );
    let accepted = runtime
        .restore_worker_operation_scoped(&scope, &worker_ref, request.clone())
        .unwrap();
    assert_eq!(accepted.state, server_api::WorkerRestoreState::Accepted);
    assert_eq!(
        runtime
            .restore_worker_operation_scoped(&scope, &worker_ref, request.clone())
            .unwrap(),
        accepted
    );
    assert_eq!(restore_owner_count(&runtime, &worker_ref), 1);
    let mut stale = request;
    stale.request_id = "terminal-fresh-stale-intent".into();
    assert!(matches!(
        runtime.restore_worker_operation_scoped(&scope, &worker_ref, stale),
        Err(crate::error::RuntimeError::RestoreObservationConflict { .. })
    ));
    assert_single_publication(&fixture, 1);
    assert_eq!(
        script.call_count.load(Ordering::SeqCst),
        0,
        "receipt recovery cannot trigger input or a model call"
    );
    // This transport was paired before the failed receipt write. Exercise its
    // normal log-entry receiver after autonomous settlement, not a new attach.
    transport
        .dispatch(Method::submit_text(
            "terminal-ordinary-input",
            "terminal-ordinary-input",
        ))
        .unwrap();
    fixture
        .backend
        .run_on_adapter_runtime(async move {
            let mut transport = transport;
            let (mut admitted, mut committed, mut text, mut ended) = (false, false, false, false);
            tokio::time::timeout(WAIT, async {
                while !(admitted && committed && text && ended) {
                    match transport.events.recv().await.unwrap() {
                        Event::SubmissionAccepted {
                            submission_request_id,
                            ..
                        } if submission_request_id == "terminal-ordinary-input" => admitted = true,
                        Event::UserMessage { segments, .. } => {
                            assert_eq!(
                                segments,
                                vec![protocol::Segment::text("terminal-ordinary-input")]
                            );
                            assert!(!committed, "paired log receiver duplicated input");
                            committed = true;
                        }
                        Event::TextDone { text: output } => {
                            assert_eq!(output, "hello from worker");
                            text = true;
                        }
                        Event::RunEnd { .. } => ended = true,
                        Event::SubmissionRejected { .. } | Event::Error { .. } => {
                            panic!("ordinary input failed after receipt recovery")
                        }
                        _ => {}
                    }
                }
            })
            .await
            .map_err(|_| {
                "ordinary input failed to settle on original paired transport".to_string()
            })?;
            drop(transport);
            Ok(())
        })
        .unwrap();
    assert_eq!(script.call_count.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.entries().iter().filter(|entry| matches!(entry,
        LogEntry::AnnotatedUserInput { segments, .. } if segments == &vec![protocol::Segment::text("terminal-ordinary-input")]
    )).count(), 1);
    assert_single_publication(&fixture, 1);
    drop(execution);
    drop(current);
    runtime
        .stop_worker_scoped(&scope, &worker_ref, None)
        .unwrap();
    assert!(old.protocol_is_closed());
    fixture.assert_released();
}

#[test]
#[serial_test::serial(worker_allocation)]
fn published_controller_terminal_receipt_before_rename_autonomously_settles_without_reconstruction()
{
    published_terminal_receipt_fault(TerminalFault::BeforeRename);
}

#[test]
#[serial_test::serial(worker_allocation)]
fn published_controller_terminal_receipt_after_rename_is_canonically_reread_and_synced() {
    published_terminal_receipt_fault(TerminalFault::AfterRename);
}

#[test]
#[serial_test::serial(worker_allocation)]
fn published_controller_terminal_receipt_sync_unknown_retains_owner_and_write_fence() {
    published_terminal_receipt_fault(TerminalFault::SyncUnknown);
}

#[test]
#[serial_test::serial(worker_allocation)]
fn published_controller_terminal_receipt_read_unknown_retains_owner_and_write_fence() {
    published_terminal_receipt_fault(TerminalFault::ReadUnknown);
}
