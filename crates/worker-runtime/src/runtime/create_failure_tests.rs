// Registered by runtime.rs's normal library test module. Execution responses are
// fixtures; creation, persistence, projection and compensation use the real owner.
use protocol::startup_failure::{
    RuntimeStartupCause, RuntimeStartupReason, StartupCleanup, StartupCleanupFailure,
    StartupIoKind, StartupResourceFacts, StartupResources, StartupStage,
};

fn boundary_reason(failure: &WorkerCreateFailure) -> StartupBoundaryReason {
    let projected = failure
        .startup()
        .primary
        .project_for::<error_contract::InternalTransfer, StartupCause>(&())
        .unwrap();
    let StartupCause::Boundary(cause) = projected.details() else {
        panic!("expected a closed boundary cause");
    };
    cause.reason
}

pub(super) fn assert_ack_failure(error: &RuntimeError) {
    assert!(matches!(error, RuntimeError::WorkerCreateFailed { .. }));
    let failure = error.create_failure().unwrap();
    assert_eq!(failure.operation().acceptance, CreationAcceptance::Accepted);
    assert_eq!(failure.operation().execution, ExecutionOutcome::Unknown);
    assert_eq!(
        failure.operation().finalize,
        Some(FinalizeFacts {
            stage: FinalizeStage::InitialInput,
            compensation: CompensationOutcome::Completed,
        })
    );
    assert_eq!(
        boundary_reason(&failure),
        StartupBoundaryReason::InvalidResponse
    );
    // The missing/foreign receipt is not rewritten into a confirmed rejection.
    assert!(error.execution_result().unwrap().is_accepted());
    assert_eq!(error.to_string(), "worker creation failed");
}

fn rejected_input() -> WorkerExecutionResult {
    let attachment = worker::startup_failure::runtime_startup_attachment(RuntimeStartupCause {
        stage: StartupStage::Controller,
        reason: RuntimeStartupReason::StartupFailed,
    })
    .unwrap();
    let mut result = WorkerExecutionResult::rejected(
        WorkerExecutionOperation::Input,
        "TEST_ONLY_SECRET original rejection /private/owner/path",
    )
    .with_startup_failure(attachment);
    result
        .secondary_failures
        .push(StartupSecondaryFailure::Cleanup {
            failure: StartupCleanupFailure::Workdir,
        });
    result.startup_resources = Some(StartupResourceFacts {
        resources: StartupResources::Unproven,
        cleanup: StartupCleanup::Failed(StartupCleanupFailure::Workdir),
        durable_effects_possible: true,
    });
    result.startup_failure_persistence = Some(StartupPersistenceOutcome::NotSaved);
    result.startup_io_kind = Some(StartupIoKind::PermissionDenied);
    result
}

#[derive(Clone, Copy, Debug)]
enum DispatchFault {
    DroppedAck,
    MismatchedAck,
    ConfirmedRejection,
    LostResponse,
}

#[derive(Clone, Copy, Debug)]
enum CleanupFault {
    None,
    MissingTermination,
    Rejected,
    LostResponse,
    MismatchedTermination,
}

#[test]
fn connected_create_dispatch_and_cleanup_fault_matrix() {
    for dispatch in [
        DispatchFault::DroppedAck,
        DispatchFault::MismatchedAck,
        DispatchFault::ConfirmedRejection,
        DispatchFault::LostResponse,
    ] {
        for cleanup in [
            CleanupFault::None,
            CleanupFault::MissingTermination,
            CleanupFault::Rejected,
            CleanupFault::LostResponse,
            CleanupFault::MismatchedTermination,
        ] {
            let (runtime, backend) = runtime_and_backend();
            let original = match dispatch {
                DispatchFault::DroppedAck => {
                    WorkerExecutionResult::accepted(WorkerExecutionOperation::Input)
                }
                DispatchFault::MismatchedAck => {
                    backend.preserve_submission_acknowledgement_id();
                    WorkerExecutionResult::accepted_submission(
                        WorkerExecutionOperation::Input,
                        "foreign-request",
                        "foreign-submission",
                        protocol::SubmissionDisposition::Started,
                    )
                }
                DispatchFault::ConfirmedRejection => rejected_input(),
                DispatchFault::LostResponse => WorkerExecutionResult::errored(
                    WorkerExecutionOperation::Input,
                    "response lost; TEST_ONLY_SECRET",
                ),
            };
            backend.set_dispatch_result(original.clone());
            let compensation = match cleanup {
                CleanupFault::None => CompensationOutcome::Completed,
                CleanupFault::MissingTermination => {
                    backend.set_stop_result(WorkerExecutionResult::accepted(
                        WorkerExecutionOperation::Stop,
                    ));
                    CompensationOutcome::Unknown
                }
                CleanupFault::Rejected => {
                    backend.set_stop_result(WorkerExecutionResult::rejected(
                        WorkerExecutionOperation::Stop,
                        "TEST_ONLY_SECRET stop rejected",
                    ));
                    CompensationOutcome::Residual
                }
                CleanupFault::LostResponse => {
                    backend.set_stop_result(WorkerExecutionResult::errored(
                        WorkerExecutionOperation::Stop,
                        "TEST_ONLY_SECRET stop response lost",
                    ));
                    CompensationOutcome::Unknown
                }
                CleanupFault::MismatchedTermination => {
                    let mut stop = WorkerExecutionResult::accepted(WorkerExecutionOperation::Stop);
                    stop.termination = Some(crate::execution::WorkerExecutionTermination {
                        scope: crate::execution::WorkerExecutionTerminationScope::Controller,
                        worker_ref: WorkerRef::new(WorkerId::now_v7()),
                        stop_operation_id: WorkerLifecycleOperationId::new(),
                        execution_id: Uuid::now_v7(),
                        shutdown_command_id: None,
                        submissions: Vec::new(),
                        feature_failures: Vec::new(),
                    });
                    backend.set_stop_result(stop);
                    CompensationOutcome::Unknown
                }
            };
            let mut request = task_request("connected create fault matrix");
            request.initial_input = Some(WorkerInput::user("start once"));
            let error = runtime.create_worker(request).unwrap_err();
            let failure = error.create_failure().unwrap();
            assert_eq!(failure.operation().acceptance, CreationAcceptance::Accepted);
            assert_eq!(
                failure.operation().execution,
                if matches!(dispatch, DispatchFault::ConfirmedRejection) {
                    ExecutionOutcome::Errored
                } else {
                    ExecutionOutcome::Unknown
                },
                "{dispatch:?}/{cleanup:?}"
            );
            assert_eq!(
                failure.operation().finalize,
                Some(FinalizeFacts {
                    stage: FinalizeStage::InitialInput,
                    compensation,
                })
            );
            assert_eq!(failure.operation().backend_compensation, None);
            assert_eq!(failure.operation().correlation, None);
            assert_eq!(error.execution_result(), Some(&original));
            assert_eq!(error.to_string(), "worker creation failed");
            if let Some(primary) = original.startup_failure.as_ref() {
                assert_eq!(&failure.startup().primary, primary);
                assert!(
                    failure
                        .startup()
                        .secondary
                        .starts_with(&original.secondary_failures)
                );
                assert_eq!(failure.startup().resources, original.startup_resources);
                assert_eq!(
                    failure.startup().persistence,
                    original.startup_failure_persistence
                );
                assert_eq!(failure.startup().io_kind, original.startup_io_kind);
                assert!(
                    std::error::Error::source(&error)
                        .unwrap()
                        .to_string()
                        .contains("TEST_ONLY_SECRET")
                );
            } else {
                assert_eq!(
                    boundary_reason(&failure),
                    if matches!(dispatch, DispatchFault::LostResponse) {
                        StartupBoundaryReason::FinalizeFailed
                    } else {
                        StartupBoundaryReason::InvalidResponse
                    }
                );
                let resources = failure.startup().resources.unwrap();
                assert!(resources.durable_effects_possible);
                assert_eq!(
                    resources.cleanup,
                    match cleanup {
                        CleanupFault::None => StartupCleanup::Confirmed,
                        CleanupFault::Rejected =>
                            StartupCleanup::Failed(StartupCleanupFailure::Ownership),
                        _ => StartupCleanup::Required,
                    }
                );
            }
            assert_eq!(*backend.stop_count.lock().unwrap(), 1);
            assert_eq!(backend.dispatched_inputs.lock().unwrap().len(), 1);
            assert!(
                backend
                    .submission_receipt_queries
                    .lock()
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                runtime.list_workers().unwrap().is_empty(),
                matches!(cleanup, CleanupFault::None)
            );
        }
    }
}

#[test]
fn independent_input_error_is_not_a_create_failure() {
    let (runtime, backend) = runtime_and_backend();
    let worker = runtime
        .create_worker(task_request("independent input"))
        .unwrap();
    let original = rejected_input();
    backend.set_dispatch_result(original.clone());
    let error = runtime
        .send_input(&worker.worker_ref, WorkerInput::user("ordinary input"))
        .unwrap_err();
    assert!(matches!(
        &error,
        RuntimeError::WorkerExecutionRejected { .. }
    ));
    assert_eq!(error.create_failure(), None);
    assert_eq!(error.execution_result(), Some(&original));
    assert_eq!(*backend.stop_count.lock().unwrap(), 0);
}

#[test]
fn connected_create_initial_input_read_failure_keeps_create_context() {
    let (runtime, backend) = runtime_and_backend();
    let mut request = task_request("initial input read failure");
    request.initial_input = Some(WorkerInput::user("never dispatched"));
    let worker_id = request.worker_id;
    *backend.before_spawn_result.lock().unwrap() = Some(Box::new({
        let runtime = runtime.clone();
        move || {
            runtime.lock().unwrap().workers.remove(&worker_id);
        }
    }));
    let error = runtime.create_worker(request).unwrap_err();
    let RuntimeError::WorkerCreateFailed { failure, source } = error else {
        panic!("post-Connected read failure must remain a create failure");
    };
    assert!(matches!(*source, RuntimeError::WorkerNotFound { .. }));
    assert_eq!(failure.operation().acceptance, CreationAcceptance::Accepted);
    assert_eq!(failure.operation().execution, ExecutionOutcome::Unknown);
    assert_eq!(
        failure.operation().finalize.unwrap().stage,
        FinalizeStage::InitialInput
    );
    assert_eq!(
        boundary_reason(&failure),
        StartupBoundaryReason::FinalizeFailed
    );
    assert!(backend.dispatched_inputs.lock().unwrap().is_empty());
    assert_eq!(*backend.stop_count.lock().unwrap(), 1);
}

#[test]
fn connected_create_publication_failure_preserves_known_input_acceptance() {
    let (runtime, backend) = runtime_and_backend();
    let mut request = task_request("publication fails after known acceptance");
    request.initial_input = Some(WorkerInput::user("accepted once"));
    let worker_ref = WorkerRef::new(request.worker_id);
    // Substitute only Connected data. The invalid backend Workdir identifier
    // reaches the real subscription projection after the owner commit.
    backend
        .contexts
        .lock()
        .unwrap()
        .insert(request.worker_id, runtime.execution_context(worker_ref));
    *backend.spawn_result.lock().unwrap() = Some(WorkerExecutionSpawnResult::Connected {
        worker_state: protocol::WorkerStatus::Idle.into(),
        workdir_attachments: vec![WorkingDirectoryAttachmentStatus {
            alias: workdir::WorkdirAttachmentAlias::new("checkout").unwrap(),
            working_directory: serde_json::from_value(serde_json::json!({
                "summary": {
                    "working_directory_id": "",
                    "repository_id": "repo",
                    "materializer_kind": "runtime_git_clone",
                    "status": "active"
                }
            }))
            .unwrap(),
        }],
    });
    let error = runtime.create_worker(request).unwrap_err();
    let failure = error.create_failure().unwrap();
    assert_eq!(failure.operation().acceptance, CreationAcceptance::Accepted);
    assert_eq!(failure.operation().execution, ExecutionOutcome::Accepted);
    assert_eq!(
        failure.operation().finalize,
        Some(FinalizeFacts {
            stage: FinalizeStage::Publication,
            compensation: CompensationOutcome::Completed,
        })
    );
    assert_eq!(
        boundary_reason(&failure),
        StartupBoundaryReason::FinalizeFailed
    );
    let RuntimeError::WorkerCreateFailed { source, .. } = error else {
        unreachable!()
    };
    assert!(matches!(*source, RuntimeError::InvalidRequest(_)));
    assert_eq!(backend.dispatched_inputs.lock().unwrap().len(), 1);
    assert_eq!(*backend.stop_count.lock().unwrap(), 1);
}

#[test]
fn create_correlation_uses_bound_runtime_identity_for_connected_and_spawn_failures() {
    for connected in [false, true] {
        let (runtime, backend) = runtime_and_backend();
        runtime.bind_runtime_identity("runtime-identity").unwrap();
        let mut request = task_request("safe correlation");
        let worker_id = request.worker_id;
        if connected {
            request.initial_input = Some(WorkerInput::user("start once"));
            backend.set_dispatch_result(rejected_input());
        } else {
            let mut result = rejected_input();
            result.operation = WorkerExecutionOperation::Spawn;
            *backend.spawn_result.lock().unwrap() =
                Some(WorkerExecutionSpawnResult::Rejected(result));
        }
        let error = runtime.create_worker(request).unwrap_err();
        assert_eq!(
            error.create_failure().unwrap().operation().correlation,
            Some(StartupCorrelation::for_worker(
                "runtime-identity",
                &worker_id.to_string()
            ))
        );
        assert_eq!(error.to_string(), "worker creation failed");
    }
}

#[cfg(feature = "fs-store")]
#[test]
fn connected_create_commit_write_faults_preserve_acceptance_and_io_facts() {
    use crate::fs_store::AtomicWriteFault;
    for fault in [
        AtomicWriteFault::BeforeRename,
        AtomicWriteFault::AfterRename,
    ] {
        for has_initial_input in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let options = FsRuntimeStoreOptions::new(root.path().join("runtime"))
                .with_runtime_id("create-commit-fault");
            let backend = Arc::new(TestExecutionBackend::default());
            let runtime =
                Runtime::with_fs_store_and_execution_backend(options.clone(), backend.clone())
                    .unwrap();
            runtime.store_config_bundle(test_bundle()).unwrap();
            *backend.before_spawn_result.lock().unwrap() = Some(Box::new({
                let runtime = runtime.clone();
                move || {
                    runtime
                        .lock()
                        .unwrap()
                        .fs_store()
                        .unwrap()
                        .fail_next_worker_write(fault)
                }
            }));
            let mut request = task_request("create commit fault");
            if has_initial_input {
                request.initial_input = Some(WorkerInput::user("accepted once"));
            }
            let worker_id = request.worker_id;
            let error = runtime.create_worker(request).unwrap_err();
            let failure = error.create_failure().unwrap();
            assert_eq!(failure.operation().acceptance, CreationAcceptance::Accepted);
            assert_eq!(failure.operation().execution, ExecutionOutcome::Accepted);
            assert_eq!(
                failure.operation().finalize,
                Some(FinalizeFacts {
                    stage: FinalizeStage::Publication,
                    compensation: CompensationOutcome::Completed,
                })
            );
            assert_eq!(
                boundary_reason(&failure),
                StartupBoundaryReason::FinalizeFailed
            );
            assert_eq!(failure.startup().io_kind, Some(StartupIoKind::Other));
            assert_eq!(failure.startup().persistence, None);
            assert!(
                failure
                    .startup()
                    .secondary
                    .contains(&StartupSecondaryFailure::Persistence {
                        phase: StartupFailurePhase::RuntimeReconciliationRecordSave,
                        outcome: if fault == AtomicWriteFault::AfterRename {
                            StartupPersistenceOutcome::OutcomeUnknown
                        } else {
                            StartupPersistenceOutcome::NotSaved
                        },
                    })
            );
            let RuntimeError::WorkerCreateFailed { source, .. } = error else {
                unreachable!()
            };
            assert!(matches!(
                *source,
                RuntimeError::StoreIo { .. } | RuntimeError::StoreCommitOutcomeUnknown { .. }
            ));
            assert_eq!(
                backend.dispatched_inputs.lock().unwrap().len(),
                usize::from(has_initial_input)
            );
            assert_eq!(*backend.stop_count.lock().unwrap(), 1);
            assert!(
                !options
                    .root
                    .join("workers")
                    .join(worker_id.to_string())
                    .exists()
            );
            drop(runtime);
            drop(backend);
            let reopened = Runtime::with_fs_store(options).unwrap();
            assert!(reopened.list_workers().unwrap().is_empty());
        }
    }
}

#[cfg(feature = "fs-store")]
#[test]
fn connected_create_cleanup_persistence_faults_never_replace_primary() {
    for delete_fault in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let store_root = root.path().join("runtime");
        let options =
            FsRuntimeStoreOptions::new(&store_root).with_runtime_id("create-cleanup-fault");
        let backend = Arc::new(TestExecutionBackend::default());
        let runtime =
            Runtime::with_fs_store_and_execution_backend(options, backend.clone()).unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        let mut request = task_request("create cleanup persistence fault");
        request.initial_input = Some(WorkerInput::user("rejected once"));
        let worker_dir = store_root
            .join("workers")
            .join(request.worker_id.to_string());
        let runtime_path = store_root.join("runtime.json");
        *backend.before_spawn_result.lock().unwrap() = Some(Box::new(move || {
            if delete_fault {
                std::fs::create_dir(worker_dir.join("metadata.json")).unwrap();
            } else {
                std::fs::rename(&runtime_path, runtime_path.with_extension("saved")).unwrap();
                std::fs::create_dir(&runtime_path).unwrap();
            }
        }));
        let original = rejected_input();
        backend.set_dispatch_result(original.clone());
        let error = runtime.create_worker(request).unwrap_err();
        let failure = error.create_failure().unwrap();
        assert_eq!(error.execution_result(), Some(&original));
        assert_eq!(failure.startup().primary, original.startup_failure.unwrap());
        assert_eq!(failure.startup().io_kind, original.startup_io_kind);
        assert_eq!(failure.operation().execution, ExecutionOutcome::Errored);
        assert_eq!(
            failure.operation().finalize.unwrap().compensation,
            CompensationOutcome::Unknown
        );
        assert!(
            failure
                .startup()
                .secondary
                .contains(&StartupSecondaryFailure::Persistence {
                    phase: if delete_fault {
                        StartupFailurePhase::RuntimeRecordDelete
                    } else {
                        StartupFailurePhase::RuntimeSnapshotSave
                    },
                    outcome: StartupPersistenceOutcome::OutcomeUnknown,
                })
        );
        if !delete_fault {
            assert!(
                failure
                    .startup()
                    .secondary
                    .contains(&StartupSecondaryFailure::Persistence {
                        phase: StartupFailurePhase::RuntimeRecordDelete,
                        outcome: StartupPersistenceOutcome::Persisted,
                    })
            );
        }
        assert_eq!(*backend.stop_count.lock().unwrap(), 1);
        assert_eq!(backend.dispatched_inputs.lock().unwrap().len(), 1);
    }
}

#[test]
fn connected_create_retains_cleanup_owner_resource_and_metadata_observations() {
    let (runtime, backend) = runtime_and_backend();
    backend.set_dispatch_result(WorkerExecutionResult::accepted(
        WorkerExecutionOperation::Input,
    ));
    let mut stop =
        WorkerExecutionResult::errored(WorkerExecutionOperation::Stop, "TEST_ONLY_SECRET");
    let resources = StartupResourceFacts {
        resources: StartupResources::Unproven,
        cleanup: StartupCleanup::Failed(StartupCleanupFailure::Feature),
        durable_effects_possible: true,
    };
    stop.startup_resources = Some(resources);
    stop.startup_io_kind = Some(StartupIoKind::TimedOut);
    stop.startup_failure_persistence = Some(StartupPersistenceOutcome::OutcomeUnknown);
    backend.set_stop_result(stop);
    let mut request = task_request("cleanup owner observations");
    request.initial_input = Some(WorkerInput::user("start once"));
    let error = runtime.create_worker(request).unwrap_err();
    let failure = error.create_failure().unwrap();
    assert_eq!(failure.startup().resources, Some(resources));
    assert_eq!(failure.startup().io_kind, Some(StartupIoKind::TimedOut));
    assert_eq!(failure.startup().persistence, None);
    assert!(
        failure
            .startup()
            .secondary
            .contains(&StartupSecondaryFailure::Persistence {
                phase: StartupFailurePhase::MetadataSave,
                outcome: StartupPersistenceOutcome::OutcomeUnknown,
            })
    );
    assert_eq!(
        failure.operation().finalize.unwrap().compensation,
        CompensationOutcome::Unknown
    );
    assert_eq!(
        boundary_reason(&failure),
        StartupBoundaryReason::InvalidResponse
    );
    assert_eq!(*backend.stop_count.lock().unwrap(), 1);
}

#[cfg(feature = "fs-store")]
#[test]
fn connected_create_unknown_cleanup_retention_write_failure_preserves_primary() {
    use crate::fs_store::AtomicWriteFault;
    let root = tempfile::tempdir().unwrap();
    let options = FsRuntimeStoreOptions::new(root.path().join("runtime"))
        .with_runtime_id("create-retain-fault");
    let backend = Arc::new(TestExecutionBackend::default());
    let runtime = Runtime::with_fs_store_and_execution_backend(options, backend.clone()).unwrap();
    runtime.store_config_bundle(test_bundle()).unwrap();
    *backend.before_spawn_result.lock().unwrap() = Some(Box::new({
        let runtime = runtime.clone();
        move || {
            runtime
                .lock()
                .unwrap()
                .fs_store()
                .unwrap()
                .fail_next_worker_write(AtomicWriteFault::BeforeRename)
        }
    }));
    let original = rejected_input();
    backend.set_dispatch_result(original.clone());
    // Accepted stop without a termination receipt cannot authorize deletion.
    backend.set_stop_result(WorkerExecutionResult::accepted(
        WorkerExecutionOperation::Stop,
    ));
    let mut request = task_request("unknown cleanup retention persistence failure");
    request.initial_input = Some(WorkerInput::user("rejected once"));
    let error = runtime.create_worker(request).unwrap_err();
    let failure = error.create_failure().unwrap();
    assert_eq!(error.execution_result(), Some(&original));
    assert_eq!(
        failure.operation().finalize.unwrap().compensation,
        CompensationOutcome::Unknown
    );
    assert!(
        failure
            .startup()
            .secondary
            .contains(&StartupSecondaryFailure::Persistence {
                phase: StartupFailurePhase::RuntimeReconciliationRecordSave,
                outcome: StartupPersistenceOutcome::NotSaved,
            })
    );
    assert_eq!(runtime.list_workers().unwrap().len(), 1);
    assert_eq!(*backend.stop_count.lock().unwrap(), 1);
    assert_eq!(backend.dispatched_inputs.lock().unwrap().len(), 1);
    assert!(
        backend
            .submission_receipt_queries
            .lock()
            .unwrap()
            .is_empty()
    );
}
