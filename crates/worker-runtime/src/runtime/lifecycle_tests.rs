#[cfg(feature = "fs-store")]
mod lifecycle_tests {
    use super::*;
    use crate::catalog::WorkerLifecycleState;
    use crate::fs_store::PersistedWorkerStopProgress;

    fn stored_runtime(options: FsRuntimeStoreOptions) -> (Runtime, Arc<TestExecutionBackend>) {
        let backend = Arc::new(TestExecutionBackend::default());
        let runtime =
            Runtime::with_fs_store_and_execution_backend(options, backend.clone()).unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        (runtime, backend)
    }

    fn assert_no_current_execution(runtime: &Runtime, worker_ref: &WorkerRef) {
        let state = runtime.lock().unwrap();
        let record = state.worker(worker_ref).unwrap();
        assert!(record.active_execution().is_none());
        assert!(record.worker_state().is_none());
        assert!(record.observation_execution().is_none());
        #[cfg(feature = "ws-server")]
        assert!(
            !state
                .subscription_worker(record)
                .unwrap()
                .has_running_internal_workers
        );
    }

    #[test]
    fn not_started_and_stopped_roundtrip_without_invented_termination() {
        for not_started in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let options =
                FsRuntimeStoreOptions::new(dir.path()).with_runtime_id("stable-lifecycle");
            let (runtime, backend) = stored_runtime(options.clone());
            let worker = runtime
                .create_worker(task_request("metadata template"))
                .unwrap();
            runtime.stop_worker(&worker.worker_ref, None).unwrap();

            // There is no public Create-without-start API. Borrow only valid
            // identity/profile metadata, then seed the canonical non-live case
            // through the actual aggregate writer, not an offline fixture schema.
            let store = runtime_store(&runtime);
            let mut saved = store.read_worker_record(&worker.worker_ref).unwrap();
            saved.restore_guard = RestoreGuard::default();
            saved.last_finished_submission_request_id = None;
            saved.execution_state = if not_started {
                PersistedWorkerExecutionState::NotStarted(None)
            } else {
                PersistedWorkerExecutionState::Stopped(None)
            };
            store.write_worker_record(&saved).unwrap();
            drop(store);
            drop(runtime);
            drop(backend);

            let reopened = Runtime::with_fs_store(options.clone()).unwrap();
            let expected = if not_started {
                WorkerLifecycleState::NotStarted
            } else {
                WorkerLifecycleState::Stopped
            };
            let detail = reopened.worker_detail(&worker.worker_ref).unwrap();
            assert_eq!(detail.lifecycle, expected);
            assert_eq!(detail.status, WorkerStatus::Stopped);
            assert!(!detail.execution_metadata_available);
            assert_no_current_execution(&reopened, &worker.worker_ref);
            assert_eq!(
                runtime_store(&reopened)
                    .read_worker_record(&worker.worker_ref)
                    .unwrap()
                    .restore_guard
                    .execution_termination,
                None
            );
            {
                let mut state = reopened.lock().unwrap();
                let record = state.worker(&worker.worker_ref).unwrap();
                assert!(record.pending_stop().is_none());
                assert!(record.pending_restore().is_none());
                assert!(record.restore_guard.stop_owners.is_empty());
                assert!(record.restore_guard.completed_stop_facts.is_empty());
                state.persist_worker(&worker.worker_id).unwrap();
            }
            let roundtrip = runtime_store(&reopened)
                .read_worker_record(&worker.worker_ref)
                .unwrap();
            assert_eq!(roundtrip.execution_state, saved.execution_state);
            assert!(roundtrip.restore_guard.execution_termination.is_none());
            let document = std::fs::read_to_string(
                options
                    .root
                    .join("workers")
                    .join(worker.worker_id.to_string())
                    .join("worker.json"),
            )
            .unwrap();
            assert!(!document.contains("\"restore_intent\""));
            drop(reopened);

            let backend = Arc::new(TestExecutionBackend::default());
            let reopened =
                Runtime::with_fs_store_and_execution_backend(options, backend.clone()).unwrap();
            assert_eq!(
                reopened
                    .worker_detail(&worker.worker_ref)
                    .unwrap()
                    .lifecycle,
                expected
            );
            assert_eq!(*backend.restore_count.lock().unwrap(), 0);
            assert_eq!(*backend.stop_count.lock().unwrap(), 0);
            assert_eq!(
                runtime_store(&reopened)
                    .read_worker_record(&worker.worker_ref)
                    .unwrap()
                    .restore_guard
                    .execution_termination,
                None
            );
        }
    }

    #[cfg(feature = "ws-server")]
    #[test]
    fn runnable_reopens_without_backend_as_awaiting_restore_but_stopped_is_not_restored() {
        let dir = tempfile::tempdir().unwrap();
        let options = FsRuntimeStoreOptions::new(dir.path()).with_runtime_id("saved-runnable");
        let (runtime, backend) = stored_runtime(options.clone());
        let runnable = runtime
            .create_worker(task_request("saved runnable"))
            .unwrap();
        let stopped = runtime
            .create_worker(task_request("explicitly stopped"))
            .unwrap();
        runtime.stop_worker(&stopped.worker_ref, None).unwrap();
        runtime
            .observe_worker_event(
                &runnable.worker_ref,
                internal_worker_status_event(
                    internal_worker_ref("before-reopen", None),
                    protocol::WorkerStatus::Running,
                ),
            )
            .unwrap();
        assert!(
            runtime
                .lock()
                .unwrap()
                .worker(&runnable.worker_ref)
                .unwrap()
                .active_execution()
                .unwrap()
                .internal_workers
                .has_running_worker()
        );
        let stopped_token = runtime
            .worker_detail(&stopped.worker_ref)
            .unwrap()
            .restore_observation_token;
        let stopped_termination = runtime.execution_termination(&stopped.worker_ref).unwrap();
        assert!(matches!(
            runtime_store(&runtime)
                .read_worker_record(&runnable.worker_ref)
                .unwrap()
                .execution_state,
            PersistedWorkerExecutionState::Runnable(_)
        ));
        drop(runtime);
        drop(backend);

        let reopened = Runtime::with_fs_store(options.clone()).unwrap();
        assert_eq!(
            reopened
                .worker_detail(&runnable.worker_ref)
                .unwrap()
                .lifecycle,
            WorkerLifecycleState::AwaitingRestore
        );
        assert!(matches!(
            reopened
                .lock()
                .unwrap()
                .worker(&runnable.worker_ref)
                .unwrap()
                .execution,
            WorkerExecutionState::AwaitingRestore(_)
        ));
        assert_no_current_execution(&reopened, &runnable.worker_ref);
        assert_no_current_execution(&reopened, &stopped.worker_ref);
        assert_eq!(
            reopened
                .worker_detail(&stopped.worker_ref)
                .unwrap()
                .lifecycle,
            WorkerLifecycleState::Stopped
        );
        drop(reopened);

        let backend = Arc::new(TestExecutionBackend::default());
        let reopened =
            Runtime::with_fs_store_and_execution_backend(options, backend.clone()).unwrap();
        assert_eq!(*backend.restore_count.lock().unwrap(), 1);
        assert_eq!(
            backend.restore_requests.lock().unwrap()[0].worker_ref,
            runnable.worker_ref
        );
        assert_eq!(
            reopened
                .worker_detail(&runnable.worker_ref)
                .unwrap()
                .lifecycle,
            WorkerLifecycleState::Active
        );
        let detail = reopened.worker_detail(&stopped.worker_ref).unwrap();
        assert_eq!(detail.lifecycle, WorkerLifecycleState::Stopped);
        assert_eq!(detail.restore_observation_token, stopped_token);
        assert_eq!(
            runtime_store(&reopened)
                .read_worker_record(&stopped.worker_ref)
                .unwrap()
                .restore_guard
                .execution_termination,
            stopped_termination
        );
        assert_no_current_execution(&reopened, &stopped.worker_ref);
        assert_eq!(*backend.stop_count.lock().unwrap(), 0);
    }

    #[test]
    fn create_persists_operation_before_spawn_instead_of_fake_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let options = FsRuntimeStoreOptions::new(dir.path()).with_runtime_id("create-before-spawn");
        let (runtime, backend) = stored_runtime(options);
        let request = task_request("durable create admission");
        let worker_ref = WorkerRef::new(request.worker_id);
        let observed_operation = Arc::new(Mutex::new(None));
        *backend.before_spawn_result.lock().unwrap() = Some(Box::new({
            let runtime = runtime.clone();
            let worker_ref = worker_ref.clone();
            let observed_operation = observed_operation.clone();
            move || {
                assert_eq!(
                    runtime.worker_detail(&worker_ref).unwrap().lifecycle,
                    WorkerLifecycleState::Creating
                );
                assert_no_current_execution(&runtime, &worker_ref);
                let saved = runtime_store(&runtime)
                    .read_worker_record(&worker_ref)
                    .unwrap();
                let PersistedWorkerExecutionState::ReconciliationRequired(
                    PersistedWorkerLifecycleOperation::Create(create),
                ) = saved.execution_state
                else {
                    panic!("spawn must follow a durable Create owner, not Stopped or Runnable")
                };
                let operation_id = create
                    .operation_id
                    .expect("new Create must have a durable operation id");
                assert!(create.request.is_some());
                assert!(saved.restore_guard.execution_termination.is_none());
                assert!(saved.restore_guard.stop_owners.is_empty());
                assert!(saved.restore_guard.completed_stop_facts.is_empty());
                let state = runtime.lock().unwrap();
                let WorkerExecutionState::Creating(current) =
                    &state.worker(&worker_ref).unwrap().execution
                else {
                    panic!("spawn candidate must still be owned by Create")
                };
                assert_eq!(current.operation_id, Some(operation_id));
                assert!(current.snapshot.is_none());
                *observed_operation.lock().unwrap() = Some(operation_id);
            }
        }));
        let detail = runtime.create_worker(request).unwrap();
        assert!(
            observed_operation.lock().unwrap().is_some(),
            "the pre-spawn hook must be reached"
        );
        assert_eq!(detail.lifecycle, WorkerLifecycleState::Active);
        assert!(matches!(
            runtime_store(&runtime)
                .read_worker_record(&worker_ref)
                .unwrap()
                .execution_state,
            PersistedWorkerExecutionState::Runnable(_)
        ));
        assert_eq!(*backend.stop_count.lock().unwrap(), 0);
    }

    #[cfg(feature = "ws-server")]
    #[test]
    fn invalidating_active_drops_snapshot_and_children_but_keeps_durable_history() {
        let dir = tempfile::tempdir().unwrap();
        let options = FsRuntimeStoreOptions::new(dir.path()).with_runtime_id("invalidate-active");
        let (runtime, backend) = stored_runtime(options.clone());
        let worker = runtime
            .create_worker(task_request("history survives invalidation"))
            .unwrap();
        let stop_request = test_stop_request(&runtime, &worker.worker_ref, "historical-stop");
        runtime
            .stop_worker_request(&worker.worker_ref, stop_request)
            .unwrap();
        let request = runtime.test_restore_request(&worker.worker_ref);
        let receipt = runtime
            .restore_worker_operation(&worker.worker_ref, request.clone())
            .unwrap();
        assert_eq!(receipt.state, WorkerRestoreState::Accepted);
        let mut snapshot: protocol::WorkerStateSnapshot = protocol::WorkerStatus::Idle.into();
        snapshot.last_finished_submission_request_id = Some("finished-before-invalidation".into());
        runtime
            .observe_worker_event(
                &worker.worker_ref,
                protocol::Event::WorkerState { snapshot },
            )
            .unwrap();
        runtime
            .observe_worker_event(
                &worker.worker_ref,
                internal_worker_status_event(
                    internal_worker_ref("current-child", None),
                    protocol::WorkerStatus::Running,
                ),
            )
            .unwrap();
        let before = runtime_store(&runtime)
            .read_worker_record(&worker.worker_ref)
            .unwrap();
        let old_token;
        {
            let mut state = runtime.lock().unwrap();
            let record = state.worker_mut(&worker.worker_ref).unwrap();
            assert!(
                record
                    .active_execution()
                    .unwrap()
                    .internal_workers
                    .has_running_worker()
            );
            assert!(record.worker_state().is_some());
            old_token = record.restore_guard.observation_token.clone();
            // T-825's owner transition seam; the lost-endpoint consumer that
            // invokes it is T-826. Exercise its real persistence/projection here.
            record.invalidate_active();
            assert_ne!(record.restore_guard.observation_token, old_token);
            assert!(matches!(
                record.execution,
                WorkerExecutionState::AwaitingRestore(_)
            ));
            state.persist_worker(&worker.worker_id).unwrap();
            state.publish_worker_upsert(worker.worker_id).unwrap();
        }
        assert_no_current_execution(&runtime, &worker.worker_ref);
        let after = runtime_store(&runtime)
            .read_worker_record(&worker.worker_ref)
            .unwrap();
        assert_eq!(after.execution_state, before.execution_state);
        assert_eq!(
            after.last_finished_submission_request_id,
            Some("finished-before-invalidation".into())
        );
        assert_eq!(
            serde_json::to_value(&after.restore_guard.owners).unwrap(),
            serde_json::to_value(&before.restore_guard.owners).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&after.restore_guard.stop_owners).unwrap(),
            serde_json::to_value(&before.restore_guard.stop_owners).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&after.restore_guard.completed_stop_facts).unwrap(),
            serde_json::to_value(&before.restore_guard.completed_stop_facts).unwrap()
        );
        assert_eq!(
            after.logical_workdir_attachments,
            before.logical_workdir_attachments
        );
        drop(runtime);
        drop(backend);

        let reopened = Runtime::with_fs_store(options).unwrap();
        assert_no_current_execution(&reopened, &worker.worker_ref);
        let state = reopened.lock().unwrap();
        let record = state.worker(&worker.worker_ref).unwrap();
        assert_eq!(record.lifecycle(), WorkerLifecycleState::AwaitingRestore);
        assert_eq!(
            record.last_finished_submission_request_id,
            after.last_finished_submission_request_id
        );
        assert_eq!(
            record.restore_guard.owners[&request.request_id]
                .receipt
                .as_ref(),
            Some(&receipt)
        );
        assert_eq!(
            serde_json::to_value(&record.restore_guard.stop_owners).unwrap(),
            serde_json::to_value(&before.restore_guard.stop_owners).unwrap()
        );
    }

    // Only the factory-owned cleanup boundary is faulted. The backend below is
    // production saved-Stop reconciliation; no Controller may be spawned again.
    struct CleanupRetryFactory {
        fail: Arc<AtomicBool>,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl crate::worker_backend::RuntimeWorkerFactory for CleanupRetryFactory {
        async fn spawn_controller(
            &self,
            _: WorkerExecutionSpawnRequest,
        ) -> Result<
            crate::worker_backend::RuntimeWorkerController,
            crate::worker_backend::RuntimeRestoreFailure,
        > {
            panic!("saved Stop cleanup must not spawn a Controller")
        }

        async fn prepare_restore(
            &self,
            _: WorkerExecutionRestoreRequest,
        ) -> Result<
            Box<dyn crate::worker_backend::RuntimeRestoreCandidate>,
            crate::worker_backend::RuntimeRestoreFailure,
        > {
            panic!("saved Stop cleanup must not prepare a replacement")
        }

        async fn reconcile_stopped_worker(&self, _: &WorkerRef) -> Result<(), String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail.load(Ordering::SeqCst) {
                Err("factory artifact cleanup still pending".into())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn scope_terminated_cleanup_failure_reopens_and_retries_same_stop_without_second_stop() {
        let dir = tempfile::tempdir().unwrap();
        let options =
            FsRuntimeStoreOptions::new(dir.path()).with_runtime_id("stop-cleanup-progress");
        let (runtime, backend) = stored_runtime(options.clone());
        let worker = runtime
            .create_worker(task_request("joined scope, pending cleanup"))
            .unwrap();
        let operation_id = WorkerLifecycleOperationId::new();
        runtime
            .begin_stop_operation(&worker.worker_ref, operation_id)
            .unwrap();
        // Substitute only the execution-owner join response, as in existing
        // execution_termination_tests. All lifecycle/save/reopen work is real.
        let mut result = backend.stop_worker_operation(WorkerExecutionStopRequest {
            worker_ref: worker.worker_ref.clone(),
            operation_id,
        });
        let termination = result.termination.clone().unwrap();
        result.outcome = crate::execution::WorkerExecutionOutcome::Errored;
        result.message = Some("artifact cleanup failed after join".into());
        assert!(matches!(
            runtime.commit_stopped_worker(&worker.worker_ref, operation_id, result),
            Err(RuntimeError::WorkerExecutionRejected { .. })
        ));
        let saved = runtime_store(&runtime)
            .read_worker_record(&worker.worker_ref)
            .unwrap();
        let PersistedWorkerExecutionState::ReconciliationRequired(
            PersistedWorkerLifecycleOperation::Stop(stop),
        ) = saved.execution_state
        else {
            panic!("cleanup failure must retain Stop, not settle Stopped")
        };
        assert_eq!(stop.operation_id, operation_id);
        assert!(
            matches!(stop.progress, PersistedWorkerStopProgress::ScopeTerminated(ref original) if original == &termination)
        );
        assert_eq!(*backend.stop_count.lock().unwrap(), 1);
        drop(runtime);

        let reopened = Runtime::with_fs_store(options.clone()).unwrap();
        assert_no_current_execution(&reopened, &worker.worker_ref);
        {
            let state = reopened.lock().unwrap();
            let record = state.worker(&worker.worker_ref).unwrap();
            assert_eq!(record.lifecycle(), WorkerLifecycleState::RecoveryRequired);
            assert_eq!(record.pending_stop().unwrap().operation_id, operation_id);
            assert!(
                matches!(&record.stop_operation().unwrap().progress, StopProgress::ScopeTerminated(original) if original == &termination)
            );
        }
        drop(reopened);

        let fail = Arc::new(AtomicBool::new(true));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let production = Arc::new(
            crate::worker_backend::WorkerRuntimeExecutionBackend::new(CleanupRetryFactory {
                fail: fail.clone(),
                calls: calls.clone(),
            })
            .unwrap(),
        );
        // Startup invokes the production factory cleanup hook, and must leave
        // the same canonical progress unresolved when that hook fails again.
        let reopened =
            Runtime::with_fs_store_and_execution_backend(options.clone(), production).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            reopened
                .worker_detail(&worker.worker_ref)
                .unwrap()
                .lifecycle,
            WorkerLifecycleState::RecoveryRequired
        );
        assert_eq!(
            reopened
                .lock()
                .unwrap()
                .worker(&worker.worker_ref)
                .unwrap()
                .pending_stop()
                .unwrap()
                .operation_id,
            operation_id
        );
        assert_eq!(
            reopened.execution_termination(&worker.worker_ref).unwrap(),
            Some(termination.clone())
        );
        fail.store(false, Ordering::SeqCst);
        {
            let operation_lock = reopened.worker_operation_lock(worker.worker_id).unwrap();
            let _guard = operation_lock.lock().unwrap();
            reopened
                .reconcile_pending_stop_under_lock(&worker.worker_ref)
                .unwrap();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(*backend.stop_count.lock().unwrap(), 1);
        assert_eq!(
            backend.stop_operation_ids.lock().unwrap().as_slice(),
            &[operation_id]
        );
        assert_eq!(
            reopened
                .worker_detail(&worker.worker_ref)
                .unwrap()
                .lifecycle,
            WorkerLifecycleState::Stopped
        );
        assert_no_current_execution(&reopened, &worker.worker_ref);
        let saved = runtime_store(&reopened)
            .read_worker_record(&worker.worker_ref)
            .unwrap();
        assert!(matches!(
            saved.execution_state,
            PersistedWorkerExecutionState::Stopped(_)
        ));
        assert_eq!(
            saved.restore_guard.execution_termination,
            Some(termination.clone())
        );
        assert_eq!(saved.restore_guard.completed_stop_facts.len(), 1);
        assert!(
            saved
                .restore_guard
                .completed_stop_facts
                .contains_key(&operation_id.to_string())
        );
        drop(reopened);
        let reopened = Runtime::with_fs_store(options).unwrap();
        assert_eq!(
            reopened
                .worker_detail(&worker.worker_ref)
                .unwrap()
                .lifecycle,
            WorkerLifecycleState::Stopped
        );
        assert_eq!(
            runtime_store(&reopened)
                .read_worker_record(&worker.worker_ref)
                .unwrap()
                .restore_guard
                .execution_termination,
            Some(termination)
        );
        assert_no_current_execution(&reopened, &worker.worker_ref);
    }

    #[test]
    fn candidate_activation_failure_persists_unresolved_restore_not_active() {
        let dir = tempfile::tempdir().unwrap();
        let options =
            FsRuntimeStoreOptions::new(dir.path()).with_runtime_id("activation-lifecycle");
        let (runtime, backend) = stored_runtime(options.clone());
        let worker = runtime
            .create_worker(task_request("activation failure"))
            .unwrap();
        runtime.stop_worker(&worker.worker_ref, None).unwrap();
        let request = runtime.test_restore_request(&worker.worker_ref);
        *backend.activation_result.lock().unwrap() = Some(Err("candidate activation fault".into()));
        let result = runtime
            .restore_worker_operation(&worker.worker_ref, request.clone())
            .unwrap();
        assert_eq!(result.state, WorkerRestoreState::ReconciliationRequired);
        assert_no_current_execution(&runtime, &worker.worker_ref);
        let operation_id;
        {
            let state = runtime.lock().unwrap();
            let record = state.worker(&worker.worker_ref).unwrap();
            assert!(matches!(
                record.lifecycle(),
                WorkerLifecycleState::Restoring | WorkerLifecycleState::RecoveryRequired
            ));
            let restore = record.restore_operation().unwrap();
            operation_id = restore.pending.operation_id;
            assert_eq!(restore.pending.commit, RestoreCommitFact::Committed);
            assert!(matches!(restore.candidate, RestoreCandidate::Prepared(_)));
            assert!(restore.candidate_context.is_some());
            assert!(
                record.restore_guard.owners[&request.request_id]
                    .receipt
                    .is_none()
            );
        }
        let saved = runtime_store(&runtime)
            .read_worker_record(&worker.worker_ref)
            .unwrap();
        let PersistedWorkerExecutionState::ReconciliationRequired(
            PersistedWorkerLifecycleOperation::Restore(restore),
        ) = saved.execution_state
        else {
            panic!("failed candidate activation must not persist Runnable")
        };
        assert_eq!(restore.operation_id, operation_id);
        assert_eq!(restore.commit, RestoreCommitFact::Committed);
        assert!(
            saved.restore_guard.owners[&request.request_id]
                .receipt
                .is_none()
        );
        assert_eq!(
            backend.activation_operation_ids.lock().unwrap().as_slice(),
            &[operation_id]
        );
        assert_eq!(*backend.restore_count.lock().unwrap(), 1);
        assert_eq!(*backend.stop_count.lock().unwrap(), 1);
        drop(runtime);

        let reopened = Runtime::with_fs_store(options).unwrap();
        assert_no_current_execution(&reopened, &worker.worker_ref);
        let state = reopened.lock().unwrap();
        let record = state.worker(&worker.worker_ref).unwrap();
        assert_eq!(record.lifecycle(), WorkerLifecycleState::RecoveryRequired);
        let restore = record.restore_operation().unwrap();
        assert_eq!(restore.pending.operation_id, operation_id);
        assert_eq!(restore.pending.commit, RestoreCommitFact::Committed);
        assert!(matches!(restore.candidate, RestoreCandidate::Unprepared));
        assert!(
            record.restore_guard.owners[&request.request_id]
                .receipt
                .is_none()
        );
    }
    #[cfg(feature = "ws-server")]
    #[test]
    fn already_connected_restore_keeps_active_snapshot_and_child_activity() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, backend) = stored_runtime(
            FsRuntimeStoreOptions::new(dir.path()).with_runtime_id("connected-noop"),
        );
        let worker = runtime
            .create_worker(task_request("already connected"))
            .unwrap();
        runtime
            .observe_worker_event(
                &worker.worker_ref,
                internal_worker_status_event(
                    internal_worker_ref("still-running", None),
                    protocol::WorkerStatus::Running,
                ),
            )
            .unwrap();
        let request = runtime.test_restore_request(&worker.worker_ref);
        let restored = runtime
            .restore_worker_operation(&worker.worker_ref, request.clone())
            .unwrap();
        assert_eq!(restored.state, WorkerRestoreState::Accepted);
        let active = runtime.worker_detail(&worker.worker_ref).unwrap();
        assert_eq!(active.lifecycle, WorkerLifecycleState::Active);
        assert!(active.worker_state.is_some());
        assert_eq!(
            restored.worker.as_ref().unwrap().lifecycle,
            WorkerLifecycleState::Active
        );
        let state = runtime.lock().unwrap();
        let record = state.worker(&worker.worker_ref).unwrap();
        assert!(
            record
                .active_execution()
                .unwrap()
                .internal_workers
                .has_running_worker()
        );
        assert!(record.pending_restore().is_none());
        assert_eq!(*backend.restore_count.lock().unwrap(), 0);
        assert_eq!(*backend.stop_count.lock().unwrap(), 0);
        drop(state);
        assert_eq!(
            runtime
                .restore_worker_operation(&worker.worker_ref, request)
                .unwrap(),
            restored
        );
    }

    #[test]
    fn unknown_create_publication_defers_shutdown_until_stop_admission_can_be_saved() {
        use crate::fs_store::AtomicWriteFault;
        let dir = tempfile::tempdir().unwrap();
        let (runtime, backend) = stored_runtime(
            FsRuntimeStoreOptions::new(dir.path()).with_runtime_id("create-admission-fence"),
        );
        *backend.before_spawn_result.lock().unwrap() = Some(Box::new({
            let runtime = runtime.clone();
            move || {
                let state = runtime.lock().unwrap();
                let store = state.fs_store().unwrap();
                store.fail_next_worker_write(AtomicWriteFault::AfterRename);
                store.fail_next_worker_reconcile_sync();
            }
        }));
        let request = task_request("uncertain creation publication");
        let worker_ref = WorkerRef::new(request.worker_id);
        assert!(runtime.create_worker(request).is_err());
        assert_eq!(*backend.stop_count.lock().unwrap(), 0);
        assert_eq!(
            runtime.worker_detail(&worker_ref).unwrap().lifecycle,
            WorkerLifecycleState::RecoveryRequired
        );
        // Canonical reconciliation now succeeds. There has been no physical Stop
        // to forget; this admission owns the first and only shutdown operation.
        runtime.stop_worker(&worker_ref, None).unwrap();
        assert_eq!(*backend.stop_count.lock().unwrap(), 1);
        assert_eq!(backend.stop_operation_ids.lock().unwrap().len(), 1);
        runtime.stop_worker(&worker_ref, None).unwrap();
        assert_eq!(*backend.stop_count.lock().unwrap(), 1);
        assert_eq!(
            runtime.worker_detail(&worker_ref).unwrap().lifecycle,
            WorkerLifecycleState::Stopped
        );
    }
    #[test]
    fn failed_create_cannot_rollback_after_stop_envelope_rejects_matching_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, backend) = stored_runtime(
            FsRuntimeStoreOptions::new(dir.path()).with_runtime_id("create-invalid-stop"),
        );
        let runtime = Arc::new(runtime);
        backend.set_dispatch_result(WorkerExecutionResult::rejected(
            WorkerExecutionOperation::Input,
            "input rejected",
        ));
        // The gate substitutes only the stop response after the durable owner is
        // admitted, so its matching operation ID is observed, not guessed.
        backend.set_stop_result(WorkerExecutionResult::accepted(
            WorkerExecutionOperation::Stop,
        ));
        let gate = Arc::new(RestoreGate::default());
        *backend.stop_gate.lock().unwrap() = Some(gate.clone());
        let mut request = task_request("invalid compensation envelope");
        request.initial_input = Some(WorkerInput::user("reject"));
        let worker_ref = WorkerRef::new(request.worker_id);
        let creating = {
            let runtime = runtime.clone();
            std::thread::spawn(move || runtime.create_worker(request))
        };
        assert!(gate.wait_for_entered(1, std::time::Duration::from_secs(5)));
        let operation_id = backend.stop_operation_ids.lock().unwrap()[0];
        let termination = crate::execution::WorkerExecutionTermination {
            scope: crate::execution::WorkerExecutionTerminationScope::Controller,
            worker_ref: worker_ref.clone(),
            stop_operation_id: operation_id,
            execution_id: uuid::Uuid::now_v7(),
            shutdown_command_id: None,
            submissions: Vec::new(),
            feature_failures: Vec::new(),
        };
        assert!(termination.confirms_scope_shutdown(&worker_ref, operation_id));
        let mut response = WorkerExecutionResult::accepted(WorkerExecutionOperation::Stop);
        response.execution_id = Some(uuid::Uuid::now_v7());
        response.termination = Some(termination);
        backend.set_stop_result(response);
        gate.release();
        let error = creating.join().unwrap().unwrap_err();
        assert_ne!(
            error
                .create_failure()
                .unwrap()
                .operation()
                .finalize
                .unwrap()
                .compensation,
            runtime_api::startup::CompensationOutcome::Completed
        );
        let state = runtime.lock().unwrap();
        let record = state.worker(&worker_ref).unwrap();
        assert_eq!(record.pending_stop().unwrap().operation_id, operation_id);
        assert!(record.stop_termination().is_none());
        assert!(record.restore_guard.completed_stop_facts.is_empty());
        drop(state);
        assert!(matches!(
            runtime_store(&runtime)
                .read_worker_record(&worker_ref)
                .unwrap()
                .execution_state,
            PersistedWorkerExecutionState::ReconciliationRequired(
                PersistedWorkerLifecycleOperation::Stop(_)
            )
        ));
    }
    #[test]
    fn rejected_preflight_does_not_invalidate_previously_active_execution() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, backend) = stored_runtime(
            FsRuntimeStoreOptions::new(dir.path()).with_runtime_id("preflight-no-effect"),
        );
        let worker = runtime
            .create_worker(task_request("preflight rejected"))
            .unwrap();
        // Substitute the preflight result without claiming that rejection is a
        // shutdown acknowledgement or permission to invalidate current data.
        backend.contexts.lock().unwrap().remove(&worker.worker_id);
        *backend.preflight_restore_result.lock().unwrap() =
            Some(WorkerExecutionResult::unsupported(
                WorkerExecutionOperation::Restore,
                "unsupported restore preflight",
            ));
        let before = runtime.worker_detail(&worker.worker_ref).unwrap();
        let result = runtime
            .restore_worker_operation(
                &worker.worker_ref,
                runtime.test_restore_request(&worker.worker_ref),
            )
            .unwrap();
        assert_eq!(result.state, WorkerRestoreState::Rejected);
        let after = runtime.worker_detail(&worker.worker_ref).unwrap();
        assert_eq!(after.lifecycle, WorkerLifecycleState::Active);
        assert_eq!(after.worker_state, before.worker_state);
        assert_eq!(*backend.restore_count.lock().unwrap(), 0);
        assert_eq!(*backend.stop_count.lock().unwrap(), 0);
    }
    #[cfg(feature = "ws-server")]
    #[test]
    fn terminal_protocol_snapshot_cannot_construct_or_select_canonical_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, backend) = stored_runtime(
            FsRuntimeStoreOptions::new(dir.path()).with_runtime_id("terminal-is-not-stop"),
        );
        let worker = runtime
            .create_worker(task_request("terminal protocol snapshot"))
            .unwrap();
        runtime
            .observe_worker_event(
                &worker.worker_ref,
                protocol::Event::WorkerState {
                    snapshot: protocol::WorkerStatus::Stopped.into(),
                },
            )
            .unwrap();
        assert_eq!(
            runtime.worker_detail(&worker.worker_ref).unwrap().lifecycle,
            WorkerLifecycleState::Active
        );
        assert!(runtime.list_stopped_workers().unwrap().is_empty());
        assert!(matches!(
            runtime.delete_worker(&worker.worker_ref),
            Err(RuntimeError::InvalidRequest(_))
        ));
        assert!(matches!(
            runtime_store(&runtime)
                .read_worker_record(&worker.worker_ref)
                .unwrap()
                .execution_state,
            PersistedWorkerExecutionState::Runnable(_)
        ));
        runtime.stop_worker(&worker.worker_ref, None).unwrap();
        assert_eq!(*backend.stop_count.lock().unwrap(), 1);
        assert_eq!(runtime.list_stopped_workers().unwrap().len(), 1);
    }
    #[cfg(feature = "ws-server")]
    #[test]
    fn create_operation_keeps_child_events_until_active_handoff() {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, backend) = stored_runtime(
            FsRuntimeStoreOptions::new(dir.path()).with_runtime_id("create-child-handoff"),
        );
        let request = task_request("child activity during create");
        let worker_ref = WorkerRef::new(request.worker_id);
        *backend.before_spawn_result.lock().unwrap() = Some(Box::new({
            let runtime = runtime.clone();
            let worker_ref = worker_ref.clone();
            move || {
                runtime
                    .observe_worker_event(
                        &worker_ref,
                        internal_worker_status_event(
                            internal_worker_ref("create-child", None),
                            protocol::WorkerStatus::Running,
                        ),
                    )
                    .unwrap();
                assert_eq!(
                    runtime.worker_detail(&worker_ref).unwrap().lifecycle,
                    WorkerLifecycleState::Creating
                );
                assert!(
                    runtime
                        .lock()
                        .unwrap()
                        .worker(&worker_ref)
                        .unwrap()
                        .active_execution()
                        .is_none()
                );
            }
        }));
        let worker = runtime.create_worker(request).unwrap();
        let state = runtime.lock().unwrap();
        let record = state.worker(&worker.worker_ref).unwrap();
        assert!(
            record
                .active_execution()
                .unwrap()
                .internal_workers
                .has_running_worker()
        );
        assert_eq!(record.lifecycle(), WorkerLifecycleState::Active);
    }
}
