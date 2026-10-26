// Included in restore_candidate: real Runtime catalog/store, Profile candidate,
// Controller and protocol; only the non-network model client and loopback
// Workspace transport are substituted. These are ordinary integration tests,
// not product-process E2E or live model generation.
#[cfg(all(feature = "fs-store", feature = "ws-server"))]
mod publication {
    use super::*;
    use crate::fs_store::{
        AtomicWriteFault, FsRuntimeStoreOptions, PersistedWorkerExecution,
        PersistedWorkerExecutionState, PersistedWorkerRecord,
    };

    fn stopped_runtime(fixture: &Fixture) -> EmbeddedRuntime {
        let options = FsRuntimeStoreOptions {
            root: fixture._root.path().join("catalog"),
            runtime_id: "candidate-runtime".into(),
            display_name: None,
        };
        let runtime = EmbeddedRuntime::with_fs_store_and_execution_backend(
            options.clone(),
            fixture.backend.clone(),
        )
        .unwrap();
        let create = fixture.request.request.clone();
        let worker_ref = fixture.request.worker_ref.clone();
        runtime
            .test_runtime_store()
            .write_worker_record(&PersistedWorkerRecord {
                restore_guard: crate::runtime::RestoreGuard::default(),
                worker_ref: worker_ref.clone(),
                worker_id: worker_ref.worker_id,
                profile: create.profile.clone(),
                display_name: None,
                profile_source: create.profile_source.reference(),
                config_bundle: create.config_bundle.clone(),
                created_at_ms: None,
                status: crate::catalog::WorkerStatus::Stopped,
                last_finished_submission_request_id: None,
                execution_state: PersistedWorkerExecutionState::Available(
                    PersistedWorkerExecution {
                        request: create,
                        binding: None,
                        restore_intent: crate::catalog::WorkerRestoreIntent::Explicit,
                    },
                ),
                workspace_id: Some(
                    fixture
                        .request
                        .workspace_scope
                        .as_ref()
                        .unwrap()
                        .workspace_id
                        .clone(),
                ),
                workdir_attachments: Vec::new(),
                logical_workdir_attachments: Vec::new(),
            })
            .unwrap();
        drop(runtime);
        EmbeddedRuntime::with_fs_store_and_execution_backend(options, fixture.backend.clone())
            .unwrap()
    }

    fn durable_operation(
        runtime: &EmbeddedRuntime,
        worker_ref: &WorkerRef,
    ) -> Option<crate::fs_store::PersistedWorkerRestoreOperation> {
        let loaded = runtime.test_runtime_store().load_runtime_state().unwrap();
        match &loaded.workers[&worker_ref.worker_id].execution_state {
            PersistedWorkerExecutionState::ReconciliationRequired(
                crate::fs_store::PersistedWorkerLifecycleOperation::Restore(operation),
            ) => Some(operation.clone()),
            _ => None,
        }
    }

    fn prepared_gate(fixture: &Fixture) -> (Arc<tokio::sync::Notify>, mpsc::Receiver<Event>) {
        let release = Arc::new(tokio::sync::Notify::new());
        let (entered, received) = mpsc::channel();
        *fixture
            .backend
            .factory
            .restore_faults
            .prepared_gate
            .lock()
            .unwrap() = Some(release.clone());
        *fixture
            .backend
            .factory
            .restore_faults
            .prepared_entered
            .lock()
            .unwrap() = Some(entered);
        (release, received)
    }

    // One-shot sender Drop is the RAII release, including a panic before the
    // explicit release. The task also has a 30s fail-safe independent of Drop.
    struct ActivationRelease(tokio::sync::oneshot::Sender<()>);

    impl ActivationRelease {
        fn release(self) {
            let _ = self.0.send(());
        }
    }

    fn activation_gate(
        fixture: &Fixture,
        started: bool,
    ) -> (ActivationRelease, mpsc::Receiver<RestoreActivationEvidence>) {
        let (release, receiver) = tokio::sync::oneshot::channel();
        let (entered, evidence) = mpsc::channel();
        let faults = &fixture.backend.factory.restore_faults;
        let slot = if started {
            &faults.activation_started_gate
        } else {
            &faults.activation_effects_gate
        };
        assert!(
            slot.lock()
                .unwrap()
                .replace(RestoreActivationGate {
                    entered,
                    release: receiver
                })
                .is_none()
        );
        (ActivationRelease(release), evidence)
    }

    fn restore_owner(
        runtime: &EmbeddedRuntime,
        worker_ref: &WorkerRef,
        request_id: &str,
    ) -> serde_json::Value {
        let record = runtime
            .test_runtime_store()
            .read_worker_record(worker_ref)
            .unwrap();
        let guard = serde_json::to_value(&record.restore_guard).unwrap();
        assert_eq!(guard["owners"].as_object().unwrap().len(), 1);
        assert!(
            guard["owners"]
                .as_object()
                .unwrap()
                .contains_key(request_id)
        );
        guard["owners"][request_id].clone()
    }

    fn assert_activation_owned(
        fixture: &Fixture,
        runtime: &EmbeddedRuntime,
        request_id: &str,
        evidence: &RestoreActivationEvidence,
    ) -> Arc<PendingRuntimeRestore> {
        let worker_ref = &fixture.request.worker_ref;
        let durable = durable_operation(runtime, worker_ref).unwrap();
        assert_eq!(durable.commit, crate::runtime::RestoreCommitFact::Committed);
        assert_eq!(durable.operation_id, evidence.operation_id);
        let owner = restore_owner(runtime, worker_ref, request_id);
        assert_eq!(
            owner["operation_id"],
            serde_json::to_value(evidence.operation_id).unwrap()
        );
        assert!(
            owner["receipt"].is_null(),
            "committed activation cannot have a terminal receipt yet"
        );
        let pending = fixture
            .backend
            .pending_restore(worker_ref)
            .unwrap()
            .unwrap();
        assert_eq!(pending.request.operation_id, evidence.operation_id);
        assert!(pending.activation_started.load(Ordering::Acquire));
        assert!(pending.task_in_flight.load(Ordering::Acquire));
        assert!(pending.task.try_lock().unwrap().is_some());
        assert!(
            pending.result.lock().unwrap().is_none(),
            "the task, not another caller, owns the candidate"
        );
        assert!(pending.activated.lock().unwrap().is_none());
        assert!(evidence.allocation_retained);
        assert!(fixture.backend.workers.lock().unwrap().is_empty());
        assert!(
            fixture
                .backend
                .factory
                .observation_hub
                .get(worker_ref)
                .is_none()
        );
        assert_eq!(
            fixture
                .backend
                .factory
                .restore_faults
                .executions
                .load(Ordering::SeqCst),
            1
        );
        fixture.assert_quiet();
        pending
    }

    fn alert_marker(handle: &WorkerHandle, marker: &str) {
        handle.alerter.alert(
            protocol::AlertLevel::Warn,
            protocol::AlertSource::Worker,
            marker.into(),
        );
    }

    fn is_marker(event: &Event, marker: &str) -> bool {
        matches!(event, Event::Alert(alert) if alert.message == marker)
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn postcommit_activation_timeout_autonomously_publishes_same_controller_and_paired_streams() {
        let mut fixture = Fixture::no_model();
        Arc::get_mut(&mut fixture.backend)
            .unwrap()
            .spawn_restore_timeout = Duration::from_millis(250);
        let runtime = stopped_runtime(&fixture);
        let worker_ref = fixture.request.worker_ref.clone();
        let scope = fixture.request.workspace_scope.as_ref().unwrap();
        let request = runtime.test_restore_request(&worker_ref);
        let (effects_release, effects_entered) = activation_gate(&fixture, false);
        let (started_release, started_entered) = activation_gate(&fixture, true);
        let observations = runtime.subscribe_worker_observation().unwrap();
        let first = runtime
            .restore_worker_operation_scoped(scope, &worker_ref, request.clone())
            .unwrap();
        assert_eq!(
            first.state,
            server_api::WorkerRestoreState::ReconciliationRequired
        );
        let effects = effects_entered.recv_timeout(WAIT).unwrap();
        assert!(effects.handle.is_none());
        let pending = assert_activation_owned(&fixture, &runtime, &request.request_id, &effects);
        let canonical = fixture.canonical_files();
        let owner = restore_owner(&runtime, &worker_ref, &request.request_id);
        effects_release.release();
        let started = started_entered.recv_timeout(WAIT).unwrap();
        let same_pending =
            assert_activation_owned(&fixture, &runtime, &request.request_id, &started);
        assert!(Arc::ptr_eq(&pending, &same_pending));
        assert_eq!(effects.candidate, started.candidate);
        assert_eq!(
            serde_json::to_value(&effects.snapshot).unwrap(),
            serde_json::to_value(&started.snapshot).unwrap()
        );
        assert_eq!(
            restore_owner(&runtime, &worker_ref, &request.request_id),
            owner
        );
        let old = started.handle.unwrap();
        assert!(!old.protocol_is_closed());
        // This live event is queued on the precommit paired receiver. A newly
        // subscribed bridge would miss it; it is not in its original snapshot.
        alert_marker(&old, "activation-paired-before-publication");
        started_release.release();
        runtime.test_wait_restore_continuation(&worker_ref, started.operation_id, WAIT);
        assert!(durable_operation(&runtime, &worker_ref).is_none());
        let settled = restore_owner(&runtime, &worker_ref, &request.request_id);
        assert_eq!(settled["operation_id"], owner["operation_id"]);
        assert_eq!(settled["receipt"]["state"], "accepted");
        let execution = fixture.backend.workers.lock().unwrap()[&worker_ref].clone();
        assert!(old.same_controller(&execution.handle));
        assert_eq!(execution.restore_operation_id, Some(started.operation_id));
        assert_eq!(fixture.backend.workers.lock().unwrap().len(), 1);
        assert_eq!(
            fixture
                .backend
                .factory
                .restore_faults
                .executions
                .load(Ordering::SeqCst),
            1
        );
        assert_eq!(
            fixture
                .backend
                .factory
                .restore_faults
                .publication_attempts
                .load(Ordering::SeqCst),
            1
        );
        let transport = fixture
            .backend
            .clone()
            .attach_worker_protocol(&worker_ref)
            .unwrap();
        transport.validate().unwrap();
        assert_eq!(
            serde_json::to_value(&transport.snapshot).unwrap(),
            serde_json::to_value(&effects.snapshot).unwrap()
        );
        fixture
            .backend
            .run_on_adapter_runtime(async move {
                let mut observations = observations;
                tokio::time::timeout(WAIT, async {
                    loop {
                        let observed = observations.recv().await.unwrap();
                        if is_marker(&observed.payload, "activation-paired-before-publication") {
                            break;
                        }
                    }
                })
                .await
                .map_err(|_| "paired receiver lost its prepublication event".to_string())?;
                Ok(())
            })
            .unwrap();
        assert!(
            fixture.canonical_files() == canonical,
            "canonical files changed unexpectedly"
        );
        drop(transport);
        drop(execution);
        drop(pending);
        drop(same_pending);
        runtime
            .stop_worker_scoped(scope, &worker_ref, None)
            .unwrap();
        assert!(old.protocol_is_closed());
        fixture.assert_released();
        fixture.assert_quiet();
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn postcommit_activation_timeout_late_nonretryable_failure_never_fakes_rollback() {
        for cleanup_fails in [false, true] {
            let mut fixture = Fixture::no_model();
            Arc::get_mut(&mut fixture.backend)
                .unwrap()
                .spawn_restore_timeout = Duration::from_millis(250);
            let runtime = stopped_runtime(&fixture);
            let worker_ref = fixture.request.worker_ref.clone();
            let scope = fixture.request.workspace_scope.as_ref().unwrap();
            let request = runtime.test_restore_request(&worker_ref);
            let (release, entered) = activation_gate(&fixture, false);
            let faults = &fixture.backend.factory.restore_faults;
            *faults.fail_at.lock().unwrap() = Some(RuntimeRestoreStage::ActivationEffects);
            faults.cleanup_fail.store(cleanup_fails, Ordering::SeqCst);
            let first = runtime
                .restore_worker_operation_scoped(scope, &worker_ref, request.clone())
                .unwrap();
            assert_eq!(
                first.state,
                server_api::WorkerRestoreState::ReconciliationRequired
            );
            let effects = entered.recv_timeout(WAIT).unwrap();
            let pending =
                assert_activation_owned(&fixture, &runtime, &request.request_id, &effects);
            let canonical = fixture.canonical_files();
            release.release();
            if cleanup_fails {
                let reason = runtime.test_wait_restore_continuation_parked(
                    &worker_ref,
                    effects.operation_id,
                    WAIT,
                );
                assert!(!reason.is_empty());
                let failure = pending
                    .activated
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .as_ref()
                    .err()
                    .unwrap()
                    .clone();
                assert_eq!(failure.stage, RuntimeRestoreStage::ActivationEffects);
                assert!(failure.durable_effects_possible);
                assert!(failure.needs_cleanup());
                assert!(matches!(failure.cleanup, RuntimeRestoreCleanup::Failed(_)));
                let durable = durable_operation(&runtime, &worker_ref).unwrap();
                assert_eq!(durable.operation_id, effects.operation_id);
                assert_eq!(durable.commit, crate::runtime::RestoreCommitFact::Committed);
                assert!(
                    restore_owner(&runtime, &worker_ref, &request.request_id)["receipt"].is_null()
                );
                assert!(Arc::ptr_eq(
                    &pending,
                    &fixture
                        .backend
                        .pending_restore(&worker_ref)
                        .unwrap()
                        .unwrap()
                ));
                assert!(fixture.backend.workers.lock().unwrap().is_empty());
                assert!(
                    fixture
                        .backend
                        .factory
                        .observation_hub
                        .get(&worker_ref)
                        .is_none()
                );
                assert!(
                    fixture
                        .backend
                        .completed_restore_failure(effects.operation_id, &worker_ref)
                        .is_none()
                );
                assert_eq!(faults.executions.load(Ordering::SeqCst), 1);
                // Obstruction proof ends here. Stop below is fixture teardown,
                // not an autonomous terminal-failure/success claim.
                faults.cleanup_fail.store(false, Ordering::SeqCst);
            } else {
                // A nonretryable candidate must be discarded, not reactivated.
                // Reconstruction belongs to the same committed operation.
                runtime.test_wait_restore_continuation(&worker_ref, effects.operation_id, WAIT);
                let settled = restore_owner(&runtime, &worker_ref, &request.request_id);
                assert_eq!(settled["receipt"]["state"], "accepted");
                assert_eq!(
                    fixture.backend.workers.lock().unwrap()[&worker_ref].restore_operation_id,
                    Some(effects.operation_id)
                );
                assert_eq!(faults.executions.load(Ordering::SeqCst), 2);
                assert_eq!(faults.publication_attempts.load(Ordering::SeqCst), 1);
                let transport = fixture
                    .backend
                    .clone()
                    .attach_worker_protocol(&worker_ref)
                    .unwrap();
                transport.validate().unwrap();
                drop(transport);
            }
            assert!(
                fixture.canonical_files() == canonical,
                "canonical files changed unexpectedly"
            );
            drop(pending);
            runtime
                .stop_worker_scoped(scope, &worker_ref, None)
                .unwrap();
            fixture.assert_released();
            fixture.assert_quiet();
        }
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn postcommit_activation_timeout_stop_remove_fence_late_cleanup_then_fresh_replacement() {
        let mut fixture = Fixture::no_model();
        Arc::get_mut(&mut fixture.backend)
            .unwrap()
            .spawn_restore_timeout = Duration::from_millis(250);
        let runtime = stopped_runtime(&fixture);
        let pause = runtime.test_pause_restore_continuation();
        let worker_ref = fixture.request.worker_ref.clone();
        let scope = fixture.request.workspace_scope.as_ref().unwrap();
        let request = runtime.test_restore_request(&worker_ref);
        let (release, entered) = activation_gate(&fixture, true);
        let first = runtime
            .restore_worker_operation_scoped(scope, &worker_ref, request.clone())
            .unwrap();
        assert_eq!(
            first.state,
            server_api::WorkerRestoreState::ReconciliationRequired
        );
        let started = entered.recv_timeout(WAIT).unwrap();
        let pending = assert_activation_owned(&fixture, &runtime, &request.request_id, &started);
        let old = started.handle.unwrap();
        let canonical = fixture.canonical_files();
        alert_marker(&old, "superseded-old-buffered");
        let stop_error = runtime
            .stop_worker_scoped(scope, &worker_ref, None)
            .unwrap_err();
        assert!(
            stop_error
                .to_string()
                .contains("restore factory is still pending"),
            "Stop must time out joining the activation task"
        );
        let remove_error = runtime
            .delete_worker_scoped(scope, &worker_ref)
            .unwrap_err();
        assert!(
            remove_error
                .to_string()
                .contains("pending stop reconciliation"),
            "Remove must be fenced by the unresolved Stop"
        );
        assert!(Arc::ptr_eq(
            &pending,
            &fixture
                .backend
                .pending_restore(&worker_ref)
                .unwrap()
                .unwrap()
        ));
        assert_ne!(
            runtime.worker_detail(&worker_ref).unwrap().status,
            crate::catalog::WorkerStatus::Stopped
        );
        assert!(restore_owner(&runtime, &worker_ref, &request.request_id)["receipt"].is_null());
        assert!(pending.task_in_flight.load(Ordering::Acquire));
        assert!(!old.protocol_is_closed());
        assert!(fixture.backend.workers.lock().unwrap().is_empty());
        assert!(
            fixture
                .backend
                .factory
                .observation_hub
                .get(&worker_ref)
                .is_none()
        );
        assert_eq!(
            fixture
                .backend
                .factory
                .restore_faults
                .executions
                .load(Ordering::SeqCst),
            1
        );
        release.release();
        drop(pause);
        runtime.test_wait_restore_continuation(
            &worker_ref,
            started.operation_id,
            Duration::from_secs(10),
        );
        assert_eq!(
            runtime.worker_detail(&worker_ref).unwrap().status,
            crate::catalog::WorkerStatus::Stopped
        );
        assert!(old.protocol_is_closed());
        assert!(!pending.task_in_flight.load(Ordering::Acquire));
        assert!(
            pending.task.try_lock().unwrap().is_none(),
            "late task must have been joined"
        );
        fixture.assert_released();
        // Confirmed Stop deletes the fixture orphan upload, not its Session,
        // metadata, or committed entries. Keep the comparison content-private.
        let mut stopped_canonical = canonical.clone();
        let orphans = canonical
            .keys()
            .filter(|path| {
                path.parent() == Some(std::path::Path::new("session/artifacts/paste"))
                    && path.to_string_lossy().ends_with(".file.json")
            })
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(orphans.len(), 1);
        stopped_canonical.remove(&orphans[0]);
        assert!(
            fixture.canonical_files() == stopped_canonical,
            "Stop must remove only the disposable orphan upload"
        );
        let superseded = restore_owner(&runtime, &worker_ref, &request.request_id);
        assert_eq!(superseded["receipt"]["state"], "superseded");
        assert!(
            runtime
                .read_worker_observation_events(
                    &worker_ref,
                    crate::observation::WorkerObservationCursor::zero()
                )
                .unwrap()
                .is_empty()
        );
        // Only a fresh request from a fresh observation may create a replacement.
        let replacement_request = runtime.test_restore_request(&worker_ref);
        assert_ne!(replacement_request.request_id, request.request_id);
        let accepted = runtime
            .restore_worker_operation_scoped(scope, &worker_ref, replacement_request)
            .unwrap();
        assert_eq!(accepted.state, server_api::WorkerRestoreState::Accepted);
        let replacement = fixture.backend.workers.lock().unwrap()[&worker_ref].clone();
        assert!(!old.same_controller(&replacement.handle));
        {
            // Even a late retirement callback identifies the actual old
            // Controller, never whichever execution now has this WorkerRef.
            let lock = fixture.backend.worker_lock(&worker_ref).unwrap();
            let _guard = lock.lock().unwrap();
            fixture
                .backend
                .factory
                .deactivate_stopped_controller(&worker_ref, &old)
                .unwrap();
            let (_, observed_state, _) = fixture
                .backend
                .factory
                .observation_hub
                .get(&worker_ref)
                .unwrap();
            assert!(Arc::ptr_eq(
                &observed_state,
                &replacement.handle.shared_state
            ));
        }
        assert_ne!(replacement.restore_operation_id, Some(started.operation_id));
        // Recovering the superseded intent must not settle or mutate the new
        // execution, even though both requests address the same WorkerRef.
        let replacement_owner = replacement.restore_operation_id;
        let replay = runtime
            .restore_worker_operation_scoped(scope, &worker_ref, request.clone())
            .unwrap();
        assert_eq!(replay.state, server_api::WorkerRestoreState::Superseded);
        let saved = runtime
            .test_runtime_store()
            .read_worker_record(&worker_ref)
            .unwrap();
        let guard = serde_json::to_value(&saved.restore_guard).unwrap();
        assert_eq!(guard["owners"][&request.request_id], superseded);
        assert_eq!(
            fixture.backend.workers.lock().unwrap()[&worker_ref].restore_operation_id,
            replacement_owner
        );
        assert_eq!(fixture.backend.workers.lock().unwrap().len(), 1);
        assert_eq!(
            fixture
                .backend
                .factory
                .restore_faults
                .executions
                .load(Ordering::SeqCst),
            2
        );
        // A diagnostic event on the retained old endpoint must never enter the
        // new bridge, and a command on that closed endpoint must not be rerouted.
        alert_marker(&old, "superseded-old-after-replacement");
        for method in [
            Method::Pause {
                command: protocol::WorkerCommandEnvelope::new(77111),
            },
            Method::Shutdown {
                command: protocol::WorkerCommandEnvelope::new(77112),
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
                    .is_err()
            );
        }
        assert_eq!(
            replacement.handle.shared_state.snapshot().state,
            protocol::WorkerState::Idle
        );
        let transport = fixture
            .backend
            .clone()
            .attach_worker_protocol(&worker_ref)
            .unwrap();
        transport.validate().unwrap();
        let mut observations = runtime.subscribe_worker_observation().unwrap();
        alert_marker(&replacement.handle, "replacement-live-barrier");
        fixture
            .backend
            .run_on_adapter_runtime(async move {
                tokio::time::timeout(WAIT, async {
                    loop {
                        let observed = observations.recv().await.unwrap();
                        assert!(!is_marker(&observed.payload, "superseded-old-buffered"));
                        assert!(!is_marker(
                            &observed.payload,
                            "superseded-old-after-replacement"
                        ));
                        if is_marker(&observed.payload, "replacement-live-barrier") {
                            break;
                        }
                    }
                })
                .await
                .map_err(|_| "replacement bridge did not progress".to_string())?;
                Ok(())
            })
            .unwrap();
        let events = runtime
            .read_worker_observation_events(
                &worker_ref,
                crate::observation::WorkerObservationCursor::zero(),
            )
            .unwrap();
        assert!(
            !events
                .iter()
                .any(|event| is_marker(&event.payload, "superseded-old-buffered")
                    || is_marker(&event.payload, "superseded-old-after-replacement"))
        );
        assert!(
            fixture.canonical_files() == stopped_canonical,
            "replacement changed canonical Session content"
        );
        drop(transport);
        drop(replacement);
        drop(pending);
        runtime
            .stop_worker_scoped(scope, &worker_ref, None)
            .unwrap();
        fixture.assert_released();
        fixture.assert_quiet();
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn postcommit_activation_timeout_gate_release_on_panic_keeps_owned_task_bounded() {
        let mut fixture = Fixture::no_model();
        Arc::get_mut(&mut fixture.backend)
            .unwrap()
            .spawn_restore_timeout = Duration::from_millis(250);
        let runtime = stopped_runtime(&fixture);
        let worker_ref = fixture.request.worker_ref.clone();
        let scope = fixture.request.workspace_scope.as_ref().unwrap();
        let request = runtime.test_restore_request(&worker_ref);
        let (release, entered) = activation_gate(&fixture, true);
        let first = runtime
            .restore_worker_operation_scoped(scope, &worker_ref, request.clone())
            .unwrap();
        assert_eq!(
            first.state,
            server_api::WorkerRestoreState::ReconciliationRequired
        );
        let started = entered.recv_timeout(WAIT).unwrap();
        let pending = assert_activation_owned(&fixture, &runtime, &request.request_id, &started);
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _release_on_unwind = release;
            panic!("intentional test-owner unwind at activation gate");
        }));
        assert!(panic.is_err());
        runtime.test_wait_restore_continuation(&worker_ref, started.operation_id, WAIT);
        assert_eq!(
            restore_owner(&runtime, &worker_ref, &request.request_id)["receipt"]["state"],
            "accepted"
        );
        assert!(!pending.task_in_flight.load(Ordering::Acquire));
        assert!(pending.task.try_lock().unwrap().is_none());
        assert!(
            started
                .handle
                .as_ref()
                .unwrap()
                .same_controller(&fixture.backend.workers.lock().unwrap()[&worker_ref].handle)
        );
        drop(pending);
        runtime
            .stop_worker_scoped(scope, &worker_ref, None)
            .unwrap();
        assert!(started.handle.unwrap().protocol_is_closed());
        fixture.assert_released();
        fixture.assert_quiet();
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn startup_fault_recovery_retains_cleanup_or_committed_reconstruction_without_repost() {
        use crate::runtime::RestoreCommitFact as F;
        for committed in [false, true] {
            let fixture = Fixture::no_model();
            let runtime = stopped_runtime(&fixture);
            let worker_ref = fixture.request.worker_ref.clone();
            let request = runtime.test_restore_request(&worker_ref);
            runtime.coordinate_worker_restore_operation(&worker_ref, runtime_api::WorkerRestoreCoordinationRequest {
                expected_observation_token: request.expected_observation_token.clone(),
                request_id: request.request_id.clone(), preparation: request.preparation.clone(),
            }, fixture.request.workspace_scope.as_ref()).unwrap();
            let operation = runtime.test_stage_restore_checkpoint(&worker_ref, if committed { F::Committed } else { F::NotCommitted }, false);
            let before = fixture.canonical_files();
            drop(runtime);
            if committed {
                *fixture.backend.factory.restore_faults.fail_at.lock().unwrap() = Some(RuntimeRestoreStage::Metadata);
            } else {
                fixture.backend.factory.restore_faults.cleanup_fail.store(true, Ordering::SeqCst);
            }
            let options = FsRuntimeStoreOptions::new(fixture._root.path().join("catalog")).with_runtime_id("candidate-runtime");
            let restarted = EmbeddedRuntime::with_fs_store_and_execution_backend(options, fixture.backend.clone()).unwrap();
            let pause = restarted.test_pause_restore_continuation();
            assert_eq!(durable_operation(&restarted, &worker_ref).unwrap().operation_id, operation);
            assert!(fixture.backend.completed_restore_failure(operation, &worker_ref).is_none());
            assert!(fixture.backend.workers.lock().unwrap().is_empty());
            *fixture.backend.factory.restore_faults.fail_at.lock().unwrap() = None;
            fixture.backend.factory.restore_faults.cleanup_fail.store(false, Ordering::SeqCst);
            drop(pause);
            restarted.test_wait_restore_continuation(&worker_ref, operation, WAIT);
            let receipt = restarted.restore_worker_operation(&worker_ref, request).unwrap();
            assert_eq!(receipt.state, if committed { server_api::WorkerRestoreState::Accepted } else { server_api::WorkerRestoreState::RolledBack });
            if committed {
                let transport = fixture.backend.clone().attach_worker_protocol(&worker_ref).unwrap();
                transport.validate().unwrap();
                drop(transport);
                restarted.stop_worker(&worker_ref, None).unwrap();
            } else {
                assert_eq!(fixture.backend.factory.restore_faults.executions.load(Ordering::SeqCst), 0);
                assert_eq!(fixture.canonical_files(), before);
            }
            fixture.assert_released();
            fixture.assert_quiet();
        }
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn startup_dispatches_saved_decisions_without_replaying_uncommitted_execution() {
        use crate::runtime::RestoreCommitFact as F;
        for (commit, repaired, expected) in [
            (F::NotCommitted, false, server_api::WorkerRestoreState::RolledBack),
            (F::NotCommitted, true, server_api::WorkerRestoreState::ReconciliationRequired),
            (F::Committed, false, server_api::WorkerRestoreState::Accepted),
            (F::LegacyUnknown, true, server_api::WorkerRestoreState::ReconciliationRequired),
        ] {
            let fixture = Fixture::no_model();
            let runtime = stopped_runtime(&fixture);
            let worker_ref = fixture.request.worker_ref.clone();
            let scope = fixture.request.workspace_scope.as_ref().unwrap();
            let request = runtime.test_restore_request(&worker_ref);
            assert!(runtime.coordinate_worker_restore_operation(&worker_ref, runtime_api::WorkerRestoreCoordinationRequest {
                expected_observation_token: request.expected_observation_token.clone(),
                request_id: request.request_id.clone(),
                preparation: request.preparation.clone(),
            }, Some(scope)).unwrap().0.is_none());
            let operation = runtime.test_stage_restore_checkpoint(&worker_ref, commit, repaired);
            let before = fixture.canonical_files();
            let run = fixture.backend.factory.worker_restore_run_dir(&worker_ref, operation).unwrap();
            std::fs::create_dir_all(&run).unwrap();
            std::fs::write(run.join("before-restart.marker"), "disposable").unwrap();
            if commit == F::NotCommitted && !repaired {
                // Fixture for an interrupted automatic restore. A rollback must
                // not leave an active/Automatic projection that restores again.
                let store = runtime.test_runtime_store();
                let mut record = store.read_worker_record(&worker_ref).unwrap();
                record.status = crate::catalog::WorkerStatus::Idle;
                let PersistedWorkerExecutionState::ReconciliationRequired(
                    crate::fs_store::PersistedWorkerLifecycleOperation::Restore(saved),
                ) = &mut record.execution_state else { panic!("pending fixture") };
                saved.mode = crate::fs_store::PersistedWorkerRestoreMode::Automatic;
                saved.last_settled_status = crate::catalog::WorkerStatus::Idle;
                saved.binding = Some(crate::fs_store::PersistedWorkerExecutionBinding {});
                store.write_worker_record(&record).unwrap();
            }
            drop(runtime);
            assert!(fixture.backend.pending_restore(&worker_ref).unwrap().is_none());
            assert!(fixture.backend.workers.lock().unwrap().is_empty());
            let options = FsRuntimeStoreOptions::new(fixture._root.path().join("catalog")).with_runtime_id("candidate-runtime");
            let restarted = EmbeddedRuntime::with_fs_store_and_execution_backend(options.clone(), fixture.backend.clone()).unwrap();
            if expected == server_api::WorkerRestoreState::ReconciliationRequired {
                let reason = restarted.test_wait_restore_continuation_parked(&worker_ref, operation, WAIT);
                assert!(!reason.is_empty());
                let pending = durable_operation(&restarted, &worker_ref).unwrap();
                assert_eq!(pending.operation_id, operation);
                assert_eq!(pending.commit, commit);
                assert_eq!(pending.source_repair_possible, repaired);
            }
            assert!(restarted.worker_detail(&worker_ref).is_ok(), "{commit:?}: {:?}", restarted.diagnostics());
            let receipt = restarted.restore_worker_operation(&worker_ref, request.clone()).unwrap();
            assert_eq!(receipt.state, expected, "{commit:?}: {receipt:?}; backend failure: {:?}", fixture.backend.completed_restore_failure(operation, &worker_ref));
            assert_eq!(fixture.backend.factory.restore_faults.executions.load(Ordering::SeqCst), usize::from(commit == F::Committed));
            if commit == F::Committed {
                let transport = fixture.backend.clone().attach_worker_protocol(&worker_ref).unwrap();
                transport.validate().unwrap();
                drop(transport);
            } else {
                assert_eq!(fixture.canonical_files(), before);
                assert!(fixture.backend.workers.lock().unwrap().is_empty());
                assert!(fixture.backend.factory.observation_hub.get(&worker_ref).is_none());
                assert_eq!(run.exists(), commit == F::LegacyUnknown);
            }
            if expected == server_api::WorkerRestoreState::RolledBack {
                assert_eq!(restarted.worker_detail(&worker_ref).unwrap().status, crate::catalog::WorkerStatus::Stopped);
                drop(restarted);
                let second = EmbeddedRuntime::with_fs_store_and_execution_backend(options, fixture.backend.clone()).unwrap();
                assert_eq!(second.restore_worker_operation_scoped(scope, &worker_ref, request).unwrap(), receipt);
                assert_eq!(fixture.backend.factory.restore_faults.executions.load(Ordering::SeqCst), 0);
            } else {
                restarted.stop_worker_scoped(scope, &worker_ref, None).unwrap();
            }
            fixture.assert_released();
            fixture.assert_quiet();
        }
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn stop_timeout_retains_autonomous_late_cleanup_and_blocks_remove_until_confirmed() {
        let mut fixture = Fixture::no_model();
        Arc::get_mut(&mut fixture.backend).unwrap().spawn_restore_timeout = Duration::from_millis(250);
        let runtime = stopped_runtime(&fixture);
        let pause = runtime.test_pause_restore_continuation();
        let worker_ref = fixture.request.worker_ref.clone();
        let scope = fixture.request.workspace_scope.as_ref().unwrap();
        let request = runtime.test_restore_request(&worker_ref);
        let before = fixture.canonical_files();
        let (release, prepared) = prepared_gate(&fixture);
        let result = runtime.restore_worker_operation_scoped(scope, &worker_ref, request.clone()).unwrap();
        prepared.recv_timeout(WAIT).unwrap();
        assert_eq!(result.state, server_api::WorkerRestoreState::ReconciliationRequired);
        let operation = durable_operation(&runtime, &worker_ref).unwrap();
        assert!(runtime.stop_worker_scoped(scope, &worker_ref, None).is_err());
        assert!(runtime.delete_worker_scoped(scope, &worker_ref).is_err());
        assert!(fixture.backend.workers.lock().unwrap().is_empty());
        release.notify_one();
        drop(pause);
        runtime.test_wait_restore_continuation(&worker_ref, operation.operation_id, WAIT);
        assert_eq!(runtime.worker_detail(&worker_ref).unwrap().status, crate::catalog::WorkerStatus::Stopped);
        assert!(durable_operation(&runtime, &worker_ref).is_none());
        assert!(fixture.backend.pending_restore(&worker_ref).unwrap().is_none());
        assert!(fixture.backend.workers.lock().unwrap().is_empty());
        assert!(fixture.backend.factory.observation_hub.get(&worker_ref).is_none());
        assert!(fixture.backend.clone().attach_worker_protocol(&worker_ref).is_err());
        let receipt = runtime.restore_worker_operation_scoped(scope, &worker_ref, request).unwrap();
        assert_eq!(receipt.state, server_api::WorkerRestoreState::Superseded);
        assert_eq!(receipt.reason_code.as_deref(), Some("worker_restore_superseded_by_stop"));
        assert_eq!(fixture.canonical_files(), before);
        fixture.assert_released();
        fixture.assert_quiet();
        assert!(runtime.delete_worker_scoped(scope, &worker_ref).unwrap().deleted);
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn autonomous_late_prepare_and_publication_settle_without_another_request() {
        for publication_failure in [false, true] {
            let mut fixture = Fixture::no_model();
            Arc::get_mut(&mut fixture.backend).unwrap().spawn_restore_timeout = Duration::from_millis(250);
            let runtime = stopped_runtime(&fixture);
            let worker_ref = fixture.request.worker_ref.clone();
            let request = runtime.test_restore_request(&worker_ref);
            let (release, prepared) = prepared_gate(&fixture);
            fixture.backend.factory.restore_faults.publication_failure.store(publication_failure, Ordering::SeqCst);
            let first = runtime.restore_worker_operation_scoped(fixture.request.workspace_scope.as_ref().unwrap(), &worker_ref, request.clone()).unwrap();
            prepared.recv_timeout(WAIT).unwrap();
            assert_eq!(first.state, server_api::WorkerRestoreState::ReconciliationRequired);
            let operation = durable_operation(&runtime, &worker_ref).unwrap();
            assert_eq!(operation.commit, crate::runtime::RestoreCommitFact::NotCommitted);
            fixture.assert_no_allocation();
            release.notify_one();
            runtime.test_wait_restore_continuation(&worker_ref, operation.operation_id, WAIT);
            let loaded = runtime.test_runtime_store().load_runtime_state().unwrap();
            let receipt = serde_json::to_value(&loaded.workers[&worker_ref.worker_id].restore_guard).unwrap();
            assert_eq!(receipt["owners"][&request.request_id]["receipt"]["state"], "accepted");
            assert!(durable_operation(&runtime, &worker_ref).is_none());
            let transport = fixture.backend.clone().attach_worker_protocol(&worker_ref).unwrap();
            transport.validate().unwrap();
            drop(transport);
            assert_eq!(fixture.backend.factory.restore_faults.executions.load(Ordering::SeqCst), 1);
            fixture.assert_quiet();
            runtime.stop_worker_scoped(fixture.request.workspace_scope.as_ref().unwrap(), &worker_ref, None).unwrap();
            fixture.assert_released();
        }
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn autonomous_cleanup_dispatch_settles_uncommitted_candidate_without_repost_or_stop() {
        let fixture = Fixture::no_model();
        let runtime = Arc::new(stopped_runtime(&fixture));
        let pause = runtime.test_pause_restore_continuation();
        let worker_ref = fixture.request.worker_ref.clone();
        let scope = fixture.request.workspace_scope.clone().unwrap();
        let request = runtime.test_restore_request(&worker_ref);
        let before = fixture.canonical_files();
        let (release, prepared) = prepared_gate(&fixture);
        let restoring = {
            let runtime = runtime.clone();
            let worker_ref = worker_ref.clone();
            let scope = scope.clone();
            let request = request.clone();
            std::thread::spawn(move || runtime.restore_worker_operation_scoped(&scope, &worker_ref, request))
        };
        prepared.recv_timeout(WAIT).unwrap();
        runtime.test_runtime_store().fail_next_worker_write(AtomicWriteFault::BeforeRename);
        fixture.backend.factory.restore_faults.cleanup_fail.store(true, Ordering::SeqCst);
        release.notify_one();
        let first = restoring.join().unwrap().unwrap();
        assert_eq!(first.state, server_api::WorkerRestoreState::ReconciliationRequired);
        let operation = durable_operation(&runtime, &worker_ref).unwrap();
        assert_eq!(operation.commit, crate::runtime::RestoreCommitFact::NotCommitted);
        let pending = fixture.backend.pending_restore(&worker_ref).unwrap().unwrap();
        assert!(matches!(pending.discard_result.lock().unwrap().as_ref(), Some(Err(_))));
        fixture.backend.factory.restore_faults.cleanup_fail.store(false, Ordering::SeqCst);
        drop(pending);
        drop(pause);
        runtime.test_wait_restore_continuation(&worker_ref, operation.operation_id, WAIT);
        let loaded = runtime.test_runtime_store().load_runtime_state().unwrap();
        let receipt = serde_json::to_value(&loaded.workers[&worker_ref.worker_id].restore_guard).unwrap();
        assert_eq!(receipt["owners"][&request.request_id]["receipt"]["state"], "rolled_back");
        assert_eq!(fixture.canonical_files(), before);
        assert_eq!(fixture.backend.factory.restore_faults.executions.load(Ordering::SeqCst), 1);
        fixture.assert_released();
        fixture.assert_quiet();
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn autonomous_unchanged_publication_obstruction_parks_without_false_terminal_outcome() {
        let fixture = Fixture::no_model();
        let runtime = stopped_runtime(&fixture);
        let worker_ref = fixture.request.worker_ref.clone();
        fixture.backend.factory.restore_faults.socket_failure.store(true, Ordering::SeqCst);
        let request = runtime.test_restore_request(&worker_ref);
        let first = runtime.restore_worker_operation_scoped(fixture.request.workspace_scope.as_ref().unwrap(), &worker_ref, request.clone()).unwrap();
        assert_eq!(first.state, server_api::WorkerRestoreState::ReconciliationRequired);
        let pending = fixture.backend.pending_restore(&worker_ref).unwrap().unwrap();
        let reason = runtime.test_wait_restore_continuation_parked(&worker_ref, pending.request.operation_id, WAIT);
        assert!(reason.contains("publication failed"), "{reason}");
        let durable = durable_operation(&runtime, &worker_ref).unwrap();
        assert_eq!(durable.commit, crate::runtime::RestoreCommitFact::Committed);
        let loaded = runtime.test_runtime_store().load_runtime_state().unwrap();
        let guard = serde_json::to_value(&loaded.workers[&worker_ref.worker_id].restore_guard).unwrap();
        assert!(guard["owners"][&request.request_id]["receipt"].is_null());
        assert_eq!(fixture.backend.factory.restore_faults.executions.load(Ordering::SeqCst), 1);
        // Initial call plus at most four backed-off continuation attempts. The
        // parked condition is observed directly, not guessed using a sleep.
        assert!(fixture.backend.factory.restore_faults.publication_attempts.load(Ordering::SeqCst) <= 5);
        assert!(fixture.backend.workers.lock().unwrap().is_empty());
        assert!(fixture.backend.factory.observation_hub.get(&worker_ref).is_none());
        drop(pending);
        // Permanent path-length obstruction: explicit fixture teardown only,
        // not the successful completion path asserted by autonomous tests.
        runtime.stop_worker_scoped(fixture.request.workspace_scope.as_ref().unwrap(), &worker_ref, None).unwrap();
        fixture.assert_released();
        fixture.assert_quiet();
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn autonomous_late_failure_settles_without_another_request() {
        let mut fixture = Fixture::no_model();
        Arc::get_mut(&mut fixture.backend).unwrap().spawn_restore_timeout = Duration::from_millis(250);
        let runtime = stopped_runtime(&fixture);
        let worker_ref = fixture.request.worker_ref.clone();
        let request = runtime.test_restore_request(&worker_ref);
        let release = Arc::new(tokio::sync::Notify::new());
        let (entered, prepared) = mpsc::channel();
        *fixture.backend.factory.restore_faults.factory_gate.lock().unwrap() = Some(release.clone());
        *fixture.backend.factory.restore_faults.factory_entered.lock().unwrap() = Some(entered);
        *fixture.backend.factory.restore_faults.fail_at.lock().unwrap() = Some(RuntimeRestoreStage::Feature);
        let before = fixture.canonical_files();
        let first = runtime.restore_worker_operation_scoped(fixture.request.workspace_scope.as_ref().unwrap(), &worker_ref, request.clone()).unwrap();
        prepared.recv_timeout(WAIT).unwrap();
        assert_eq!(first.state, server_api::WorkerRestoreState::ReconciliationRequired);
        let operation = durable_operation(&runtime, &worker_ref).unwrap();
        release.notify_one();
        runtime.test_wait_restore_continuation(&worker_ref, operation.operation_id, WAIT);
        let loaded = runtime.test_runtime_store().load_runtime_state().unwrap();
        let receipt = serde_json::to_value(&loaded.workers[&worker_ref.worker_id].restore_guard).unwrap();
        assert_eq!(receipt["owners"][&request.request_id]["receipt"]["state"], "rolled_back");
        assert_eq!(fixture.canonical_files(), before);
        assert_eq!(fixture.backend.factory.restore_faults.executions.load(Ordering::SeqCst), 1);
        fixture.assert_released();
        fixture.assert_quiet();
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn runtime_accepted_follows_real_prepared_snapshot_commit_attach_and_input() {
        let bootstrap_backend;
        let mut fixture = Fixture::no_model();
        let script = MockClient::sequential(vec![MockResponse::Complete(simple_text_events())]);
        let injected = script.clone();
        let factory = fixture
            .backend
            .factory
            .as_ref()
            .clone()
            .with_spawn_model_client_factory(move || Box::new(injected.clone()));
        bootstrap_backend = std::mem::replace(
            &mut fixture.backend,
            Arc::new(WorkerRuntimeExecutionBackend::new(factory).unwrap()),
        );
        let runtime = Arc::new(stopped_runtime(&fixture));
        let worker_ref = fixture.request.worker_ref.clone();
        let scope = fixture.request.workspace_scope.clone().unwrap();
        let request = runtime.test_restore_request(&worker_ref);
        let before = fixture.canonical_files();
        let (release, prepared) = prepared_gate(&fixture);
        let restoring = {
            let runtime = runtime.clone();
            let worker_ref = worker_ref.clone();
            let scope = scope.clone();
            let request = request.clone();
            std::thread::spawn(move || {
                runtime.restore_worker_operation_scoped(&scope, &worker_ref, request)
            })
        };
        let snapshot = prepared.recv_timeout(WAIT).unwrap();
        assert!(
            matches!(&snapshot, Event::Snapshot { session, greeting, state, in_flight, .. }
            if session == &session_store::public_snapshot::project_current_session_snapshot(&fixture.entries())
                && !greeting.tools.is_empty() && state.state == protocol::WorkerState::Idle && in_flight.is_empty())
        );
        let pending = durable_operation(&runtime, &worker_ref).unwrap();
        assert_eq!(
            pending.commit,
            crate::runtime::RestoreCommitFact::NotCommitted
        );
        assert_eq!(fixture.canonical_files(), before);
        fixture.assert_quiet();
        fixture.assert_no_allocation();
        assert!(fixture.backend.workers.lock().unwrap().is_empty());
        assert!(
            fixture
                .backend
                .factory
                .observation_hub
                .get(&worker_ref)
                .is_none()
        );
        // External attach shares the in-flight backend operation lock; do not
        // synchronously acquire that lock while this test holds its prepare gate.
        // The empty execution registry and absent hub/allocation prove private
        // ownership here; attach is exercised immediately after Accepted below.
        assert_eq!(script.call_count.load(Ordering::SeqCst), 0);
        release.notify_one();
        let accepted = restoring.join().unwrap().unwrap();
        assert_eq!(
            accepted.state,
            server_api::WorkerRestoreState::Accepted,
            "{accepted:?}"
        );
        assert!(durable_operation(&runtime, &worker_ref).is_none());
        let loaded = runtime.test_runtime_store().load_runtime_state().unwrap();
        let guard =
            serde_json::to_value(&loaded.workers[&worker_ref.worker_id].restore_guard).unwrap();
        assert!(!guard["owners"][&request.request_id]["receipt"].is_null());
        assert_eq!(
            fixture
                .backend
                .factory
                .restore_faults
                .executions
                .load(Ordering::SeqCst),
            1
        );
        assert!(
            fixture
                .backend
                .factory
                .observation_hub
                .get(&worker_ref)
                .is_some()
        );
        let mut transport = fixture
            .backend
            .clone()
            .attach_worker_protocol(&worker_ref)
            .unwrap();
        transport.validate().unwrap();
        assert_eq!(
            serde_json::to_value(&transport.snapshot).unwrap(),
            serde_json::to_value(snapshot).unwrap()
        );
        assert!(matches!(
            fixture
                .backend
                .worker_session(crate::execution::WorkerSessionObservationRequest {
                    worker_ref: worker_ref.clone(),
                }),
            runtime_api::WorkerSessionAvailability::LiveProtocol
        ));
        transport
            .dispatch(Method::submit_text(
                "vertical-body-input",
                "vertical-body-input",
            ))
            .unwrap();
        fixture
            .backend
            .run_on_adapter_runtime(async move {
                let (mut admitted, mut committed, mut text, mut ended) =
                    (false, false, false, false);
                tokio::time::timeout(WAIT, async {
                    while !(admitted && committed && text && ended) {
                        match transport.events.recv().await.unwrap() {
                            Event::SubmissionAccepted {
                                submission_request_id,
                                ..
                            } if submission_request_id == "vertical-body-input" => admitted = true,
                            Event::UserMessage { segments, .. } => {
                                assert_eq!(
                                    segments,
                                    vec![protocol::Segment::text("vertical-body-input")]
                                );
                                committed = true;
                            }
                            Event::TextDone { text: output } => {
                                assert_eq!(output, "hello from worker");
                                text = true;
                            }
                            Event::RunEnd { .. } => ended = true,
                            Event::SubmissionRejected { .. } | Event::Error { .. } => {
                                panic!("normal input rejected after Accepted")
                            }
                            _ => {}
                        }
                    }
                })
                .await
                .map_err(|_| "normal input did not settle".to_string())?;
                // Capture and release the whole transport, not only its events field;
                // its dispatch/validation closures retain the actual execution.
                drop(transport);
                Ok(())
            })
            .unwrap();
        assert!(fixture.entries().iter().any(
            |entry| matches!(entry, LogEntry::AnnotatedUserInput { segments, .. }
            if segments == &vec![protocol::Segment::text("vertical-body-input")])
        ));
        assert_eq!(script.call_count.load(Ordering::SeqCst), 1);
        let replay = runtime
            .restore_worker_operation_scoped(&scope, &worker_ref, request.clone())
            .unwrap();
        assert_eq!(replay.state, server_api::WorkerRestoreState::Accepted);
        assert_eq!(
            fixture
                .backend
                .factory
                .restore_faults
                .executions
                .load(Ordering::SeqCst),
            1
        );
        let mut stale = request;
        stale.request_id = "vertical-new-stale-intent".into();
        assert!(
            matches!(runtime.restore_worker_operation_scoped(&scope, &worker_ref, stale), Err(crate::error::RuntimeError::RestoreObservationConflict { worker_id }) if worker_id == worker_ref.worker_id)
        );
        runtime
            .stop_worker_scoped(&scope, &worker_ref, None)
            .unwrap();
        fixture.assert_released();
        assert_eq!(script.call_count.load(Ordering::SeqCst), 1);
        drop(runtime);
        drop(bootstrap_backend);
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn decision_write_faults_preserve_prepared_owner_or_confirm_uncommitted_cleanup() {
        for (fault, sync_unknown) in [
            (AtomicWriteFault::BeforeRename, false),
            (AtomicWriteFault::AfterRename, false),
            (AtomicWriteFault::AfterRename, true),
        ] {
            let fixture = Fixture::no_model();
            let runtime = Arc::new(stopped_runtime(&fixture));
            let _pause = runtime.test_pause_restore_continuation();
            let worker_ref = fixture.request.worker_ref.clone();
            let scope = fixture.request.workspace_scope.clone().unwrap();
            let request = runtime.test_restore_request(&worker_ref);
            let before = fixture.canonical_files();
            let (release, prepared) = prepared_gate(&fixture);
            let restoring = {
                let runtime = runtime.clone();
                let worker_ref = worker_ref.clone();
                let scope = scope.clone();
                let request = request.clone();
                std::thread::spawn(move || {
                    runtime.restore_worker_operation_scoped(&scope, &worker_ref, request)
                })
            };
            prepared.recv_timeout(WAIT).unwrap();
            let operation = durable_operation(&runtime, &worker_ref).unwrap();
            assert_eq!(
                operation.commit,
                crate::runtime::RestoreCommitFact::NotCommitted
            );
            runtime.test_runtime_store().fail_next_worker_write(fault);
            if sync_unknown {
                runtime
                    .test_runtime_store()
                    .fail_next_worker_reconcile_sync();
            }
            release.notify_one();
            let result = restoring.join().unwrap().unwrap();
            match fault {
                AtomicWriteFault::BeforeRename => {
                    assert_eq!(
                        result.state,
                        server_api::WorkerRestoreState::RolledBack,
                        "{result:?}"
                    );
                    assert_eq!(fixture.canonical_files(), before);
                    fixture.assert_released();
                }
                AtomicWriteFault::AfterRename if !sync_unknown => {
                    assert_eq!(
                        result.state,
                        server_api::WorkerRestoreState::Accepted,
                        "{result:?}"
                    );
                    let transport = fixture
                        .backend
                        .clone()
                        .attach_worker_protocol(&worker_ref)
                        .unwrap();
                    transport.validate().unwrap();
                    drop(transport);
                    runtime
                        .stop_worker_scoped(&scope, &worker_ref, None)
                        .unwrap();
                    fixture.assert_released();
                }
                AtomicWriteFault::AfterRename => {
                    assert_eq!(
                        result.state,
                        server_api::WorkerRestoreState::ReconciliationRequired,
                        "{result:?}"
                    );
                    assert_eq!(
                        durable_operation(&runtime, &worker_ref)
                            .unwrap()
                            .operation_id,
                        operation.operation_id
                    );
                    assert_eq!(
                        durable_operation(&runtime, &worker_ref).unwrap().commit,
                        crate::runtime::RestoreCommitFact::Committed
                    );
                    assert!(fixture.backend.workers.lock().unwrap().is_empty());
                    fixture.assert_no_allocation();
                    assert_eq!(fixture.canonical_files(), before);
                    let replay = runtime
                        .restore_worker_operation_scoped(&scope, &worker_ref, request)
                        .unwrap();
                    assert_eq!(
                        replay.state,
                        server_api::WorkerRestoreState::Accepted,
                        "{replay:?}"
                    );
                    assert_eq!(
                        fixture
                            .backend
                            .factory
                            .restore_faults
                            .executions
                            .load(Ordering::SeqCst),
                        1
                    );
                    runtime
                        .stop_worker_scoped(&scope, &worker_ref, None)
                        .unwrap();
                    fixture.assert_released();
                }
                _ => unreachable!(),
            }
            fixture.assert_quiet();
        }
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn uncommitted_cleanup_failure_fences_discarding_candidate_from_activation() {
        let fixture = Fixture::no_model();
        let runtime = Arc::new(stopped_runtime(&fixture));
        let _pause = runtime.test_pause_restore_continuation();
        let worker_ref = fixture.request.worker_ref.clone();
        let scope = fixture.request.workspace_scope.clone().unwrap();
        let request = runtime.test_restore_request(&worker_ref);
        let before = fixture.canonical_files();
        let (release, prepared) = prepared_gate(&fixture);
        let restoring = {
            let runtime = runtime.clone();
            let worker_ref = worker_ref.clone();
            let scope = scope.clone();
            let request = request.clone();
            std::thread::spawn(move || {
                runtime.restore_worker_operation_scoped(&scope, &worker_ref, request)
            })
        };
        prepared.recv_timeout(WAIT).unwrap();
        runtime.test_runtime_store().fail_next_worker_write(AtomicWriteFault::BeforeRename);
        fixture.backend.factory.restore_faults.cleanup_fail.store(true, Ordering::SeqCst);
        release.notify_one();
        let failed = restoring.join().unwrap().unwrap();
        assert_eq!(failed.state, server_api::WorkerRestoreState::ReconciliationRequired, "{failed:?}");
        let pending = fixture.backend.pending_restore(&worker_ref).unwrap().unwrap();
        assert!(matches!(pending.discard_result.lock().unwrap().as_ref(), Some(Err(_))));
        assert_eq!(durable_operation(&runtime, &worker_ref).unwrap().commit, crate::runtime::RestoreCommitFact::NotCommitted);
        for cleanup_fails in [true, false] {
            fixture.backend.factory.restore_faults.cleanup_fail.store(cleanup_fails, Ordering::SeqCst);
            let replay = runtime.restore_worker_operation_scoped(&scope, &worker_ref, request.clone()).unwrap();
            assert_eq!(replay.state, server_api::WorkerRestoreState::ReconciliationRequired, "discarding candidate must not activate: {replay:?}");
            assert!(fixture.backend.workers.lock().unwrap().is_empty());
            assert!(fixture.backend.factory.observation_hub.get(&worker_ref).is_none());
            assert!(fixture.backend.clone().attach_worker_protocol(&worker_ref).is_err());
            assert_eq!(fixture.backend.factory.restore_faults.executions.load(Ordering::SeqCst), 1);
            assert_eq!(fixture.canonical_files(), before);
            fixture.assert_no_allocation();
            fixture.assert_quiet();
        }
        // Explicit teardown verifies this retained owner's cleanup. Autonomous
        // cleanup convergence is a separate operation-driver contract.
        drop(pending);
        runtime.stop_worker_scoped(&scope, &worker_ref, None).unwrap();
        fixture.assert_released();
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn publication_failure_retains_same_committed_candidate_without_reconstruction() {
        let fixture = Fixture::no_model();
        let runtime = stopped_runtime(&fixture);
        let _pause = runtime.test_pause_restore_continuation();
        let worker_ref = fixture.request.worker_ref.clone();
        let scope = fixture.request.workspace_scope.clone().unwrap();
        let request = runtime.test_restore_request(&worker_ref);
        fixture
            .backend
            .factory
            .restore_faults
            .publication_failure
            .store(true, Ordering::SeqCst);
        let failed = runtime
            .restore_worker_operation_scoped(&scope, &worker_ref, request.clone())
            .unwrap();
        assert_eq!(
            failed.state,
            server_api::WorkerRestoreState::ReconciliationRequired,
            "{failed:?}"
        );
        let pending = fixture
            .backend
            .pending_restore(&worker_ref)
            .unwrap()
            .unwrap();
        assert!(pending.owns_prepared_candidate());
        assert!(pending.activation_started.load(Ordering::SeqCst));
        assert!(
            pending
                .result
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .can_retry_publication()
        );
        let durable = durable_operation(&runtime, &worker_ref).unwrap();
        assert_eq!(durable.operation_id, pending.request.operation_id);
        assert_eq!(durable.commit, crate::runtime::RestoreCommitFact::Committed);
        assert!(fixture.backend.workers.lock().unwrap().is_empty());
        assert!(
            fixture
                .backend
                .factory
                .observation_hub
                .get(&worker_ref)
                .is_none()
        );
        assert!(
            fixture
                .backend
                .clone()
                .attach_worker_protocol(&worker_ref)
                .is_err()
        );
        fixture.assert_quiet();
        let result = runtime
            .restore_worker_operation_scoped(&scope, &worker_ref, request)
            .unwrap();
        assert_eq!(
            result.state,
            server_api::WorkerRestoreState::Accepted,
            "{result:?}"
        );
        let execution = fixture.backend.workers.lock().unwrap()[&worker_ref].clone();
        assert_eq!(execution.restore_operation_id, Some(durable.operation_id));
        assert_eq!(
            fixture
                .backend
                .factory
                .restore_faults
                .executions
                .load(Ordering::SeqCst),
            1
        );
        let transport = fixture
            .backend
            .clone()
            .attach_worker_protocol(&worker_ref)
            .unwrap();
        transport.validate().unwrap();
        drop(transport);
        drop(execution);
        drop(pending);
        runtime
            .stop_worker_scoped(&scope, &worker_ref, None)
            .unwrap();
        fixture.assert_released();
        fixture.assert_quiet();
    }
}
