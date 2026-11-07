mod create_reconciliation_tests {
    //! The real create retention and FsRuntimeStore write/reopen path; only spawn
    //! disposition and the existing deterministic atomic-write hook are substituted.
    use super::*;
    use crate::fs_store::AtomicWriteFault;

    fn check_reconciliation(fault: AtomicWriteFault) {
        for has_state in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let options = FsRuntimeStoreOptions::new(root.path().join("runtime"))
                .with_runtime_id("create-reconciliation");
            let backend = Arc::new(TestExecutionBackend::default());
            let runtime =
                Runtime::with_fs_store_and_execution_backend(options.clone(), backend.clone())
                    .unwrap();
            runtime.store_config_bundle(test_bundle()).unwrap();
            let request = task_request("reconciliation failure");
            let worker_ref = WorkerRef::new(request.worker_id);
            let primary = primary_failure();
            *backend.spawn_result.lock().unwrap() =
                Some(WorkerExecutionSpawnResult::ReconciliationRequired {
                    result: primary.clone(),
                    worker_state: has_state.then(|| protocol::WorkerStatus::Stopped.into()),
                    workdir_attachments: Vec::new(),
                });
            let initial = Arc::new(Mutex::new(None));
            *backend.before_spawn_result.lock().unwrap() = Some(Box::new({
                let store = runtime_store(&runtime);
                let initial = initial.clone();
                let worker_ref = worker_ref.clone();
                move || {
                    *initial.lock().unwrap() = Some(store.read_worker_record(&worker_ref).unwrap());
                    store.fail_next_worker_write(fault);
                }
            }));
            let RuntimeError::WorkerExecutionRejected { result, .. } =
                runtime.create_worker(request).unwrap_err()
            else {
                panic!("retention failure must not replace primary result")
            };
            assert_eq!(result.startup_failure, primary.startup_failure);
            assert_eq!(result.startup_resources, primary.startup_resources);
            assert_eq!(result.outcome, primary.outcome);
            assert!(
                result
                    .secondary_failures
                    .contains(&primary.secondary_failures[0])
            );
            let outcome = match fault {
                AtomicWriteFault::BeforeRename => StartupPersistenceOutcome::NotSaved,
                AtomicWriteFault::AfterRename => StartupPersistenceOutcome::OutcomeUnknown,
                AtomicWriteFault::None => StartupPersistenceOutcome::Persisted,
            };
            assert!(result.secondary_failures.iter().any(|failure| matches!(failure, StartupSecondaryFailure::Persistence { phase: StartupFailurePhase::RuntimeReconciliationRecordSave, outcome: observed } if *observed == outcome)), "retention persistence must be explicit: {result:?}");
            let disk = runtime_store(&runtime)
                .read_worker_record(&worker_ref)
                .unwrap();
            let initial = initial.lock().unwrap().take().unwrap();
            if fault == AtomicWriteFault::BeforeRename {
                assert_eq!(disk.execution_state, initial.execution_state);
                assert_eq!(
                    disk.restore_guard.observation_token,
                    initial.restore_guard.observation_token
                );
            } else {
                assert_ne!(
                    disk.restore_guard.observation_token,
                    initial.restore_guard.observation_token
                );
                if has_state {
                    assert!(
                        matches!(disk.execution_state, crate::fs_store::PersistedWorkerExecutionState::Available(ref execution) if execution.binding.is_some())
                    );
                } else {
                    assert_eq!(
                        disk.execution_state,
                        crate::fs_store::PersistedWorkerExecutionState::Unavailable
                    );
                }
            }
            // A pre-rename failure left the initial record; an uncertain response
            // here followed a real rename and is reconciled from disk, not guessed.
            let on_disk = std::fs::read(
                root.path()
                    .join("runtime/workers")
                    .join(worker_ref.worker_id.to_string())
                    .join("worker.json"),
            )
            .unwrap();
            drop(runtime);
            drop(backend);
            let reopened = Runtime::with_fs_store(options).unwrap();
            let reread = runtime_store(&reopened)
                .read_worker_record(&worker_ref)
                .unwrap();
            assert_eq!(reread.execution_state, disk.execution_state);
            assert!(reopened.worker_detail(&worker_ref).is_ok());
            assert!(!on_disk.is_empty());
        }
    }
    #[test]
    fn create_reconciliation_write_failure_retains_primary_and_unsaved_evidence() {
        check_reconciliation(AtomicWriteFault::BeforeRename);
    }
    #[test]
    fn create_reconciliation_unknown_write_retains_primary_and_reopens_actual_record() {
        check_reconciliation(AtomicWriteFault::AfterRename);
    }
    #[test]
    fn create_reconciliation_success_retains_primary_and_durable_evidence() {
        check_reconciliation(AtomicWriteFault::None);
    }
}
