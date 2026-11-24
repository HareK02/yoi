#[cfg(feature = "fs-store")]
mod trusted_removal_tests {
    //! Component integration through Runtime deletion/retention/freeze into the
    //! real aggregate and Session stores, isolated in RAII temporary directories.
    //! Only the execution-owner response is substituted by TestExecutionBackend;
    //! these tests do not claim actual Controller shutdown/join coverage. Saved
    //! receipt fixtures test the freeze consumer, not receipt production. Run in
    //! the normal worker-runtime lib target with the fs-store product feature.
    use super::*;
    use crate::catalog::WorkerLifecycleState;
    use crate::retention::{DiagnosticsDisposition, SessionDisposition};
    use session_store::{Store, WorkerMetadataStore};

    fn options(root: &std::path::Path) -> FsRuntimeStoreOptions {
        FsRuntimeStoreOptions::new(root).with_runtime_id("trusted-removal-runtime")
    }

    fn stored_runtime(root: &std::path::Path) -> (Runtime, Arc<TestExecutionBackend>) {
        let backend = Arc::new(TestExecutionBackend::default());
        let runtime =
            Runtime::with_fs_store_and_execution_backend(options(root), backend.clone()).unwrap();
        runtime
            .bind_runtime_identity("trusted-removal-runtime")
            .unwrap();
        runtime.store_config_bundle(test_bundle()).unwrap();
        (runtime, backend)
    }

    fn scope() -> RuntimeWorkspaceScope {
        RuntimeWorkspaceScope::new("trusted-removal-workspace", "trusted-removal-server")
    }

    fn create(runtime: &Runtime) -> WorkerDetail {
        runtime
            .create_worker_scoped(
                &scope(),
                scoped_task_request("trusted removal", "trusted-removal-workspace"),
            )
            .unwrap()
    }

    fn plan(worker_id: WorkerId) -> WorkerRetentionExecutionRequest {
        WorkerRetentionExecutionRequest {
            operation_id: "trusted-removal-operation".into(),
            input_fingerprint: "trusted-removal-input".into(),
            archive_id: None,
            workspace_id: scope().workspace_id,
            source_runtime_id: "trusted-removal-runtime".into(),
            worker_id,
            source_created_at: "2026-01-01T00:00:00Z".into(),
            removed_at: "2026-01-01T01:00:00Z".into(),
            effective_profile: None,
            retention_class: None,
            policy_id: "trusted-removal-policy".into(),
            session_disposition: SessionDisposition::Purge,
            diagnostics_disposition: DiagnosticsDisposition::Purge,
        }
    }

    fn worker_root(root: &std::path::Path, worker: WorkerId) -> std::path::PathBuf {
        root.join("workers").join(worker.to_string())
    }

    fn freeze_path(root: &std::path::Path, worker: WorkerId) -> std::path::PathBuf {
        root.join("retention/removal-freezes")
            .join(format!("{worker}.json"))
    }

    fn assert_removed(runtime: &Runtime, root: &std::path::Path, worker: &WorkerDetail) {
        assert!(matches!(
            runtime.worker_detail(&worker.worker_ref),
            Err(RuntimeError::WorkerNotFound { worker_id }) if worker_id == worker.worker_id
        ));
        assert!(!worker_root(root, worker.worker_id).exists());
    }

    #[test]
    fn cold_settled_workers_delete_and_retain_without_execution_owner_or_stop_history() {
        for lifecycle in [
            "stopped-with-create-metadata",
            "stopped-without-create-metadata",
            "not-started",
        ] {
            for retention in [false, true] {
                let root = tempfile::tempdir().unwrap();
                let (runtime, backend) = stored_runtime(root.path());
                let worker = create(&runtime);
                runtime.stop_worker(&worker.worker_ref, None).unwrap();
                let store = runtime_store(&runtime);
                let mut saved = store.read_worker_record(&worker.worker_ref).unwrap();
                // No Create-without-start API exists. Keep writer-produced
                // identity/profile metadata, but seed valid canonical inactive
                // states with no historical Stop evidence, then cold reopen.
                saved.restore_guard = RestoreGuard::default();
                saved.last_finished_submission_request_id = None;
                saved.execution_state = match lifecycle {
                    "stopped-with-create-metadata" => saved.execution_state,
                    "stopped-without-create-metadata" => {
                        PersistedWorkerExecutionState::Stopped(None)
                    }
                    "not-started" => PersistedWorkerExecutionState::NotStarted(None),
                    _ => unreachable!(),
                };
                store.write_worker_record(&saved).unwrap();
                drop(store);
                drop(runtime);
                drop(backend);
                if lifecycle != "not-started" {
                    // Frozen schema-17 envelopes feed the actual migration on
                    // cold open; consumers never classify old schemas themselves.
                    let path = worker_root(root.path(), worker.worker_id).join("worker.json");
                    let mut document: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                    document["schema_version"] = serde_json::json!(17);
                    if lifecycle == "stopped-with-create-metadata" {
                        document["execution_state"]["state"] = serde_json::json!("available");
                        // This field belongs only to the frozen pre-migration input.
                        document["execution_state"]["execution"]["restore_intent"] =
                            serde_json::json!("explicit");
                    } else {
                        document["execution_state"] = serde_json::json!({"state": "unavailable"});
                    }
                    std::fs::write(path, serde_json::to_vec(&document).unwrap()).unwrap();
                    let path = root.path().join("runtime.json");
                    let mut document: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                    document["schema_version"] = serde_json::json!(17);
                    std::fs::write(path, serde_json::to_vec(&document).unwrap()).unwrap();
                }

                // An attached backend with no previous execution or receipt
                // must not be asked to stop/reconcile this settled catalog row.
                let backend = Arc::new(TestExecutionBackend::default());
                backend.set_stop_result(WorkerExecutionResult::unsupported(
                    WorkerExecutionOperation::Stop,
                    "cold settled Worker has no execution owner",
                ));
                let reopened = Runtime::with_fs_store_and_execution_backend(
                    options(root.path()),
                    backend.clone(),
                )
                .unwrap();
                reopened
                    .bind_runtime_identity("trusted-removal-runtime")
                    .unwrap();
                let detail = reopened.worker_detail(&worker.worker_ref).unwrap();
                assert_eq!(
                    detail.lifecycle,
                    if lifecycle == "not-started" {
                        WorkerLifecycleState::NotStarted
                    } else {
                        WorkerLifecycleState::Stopped
                    }
                );
                assert_eq!(
                    detail.execution_metadata_available,
                    lifecycle == "stopped-with-create-metadata"
                );
                assert!(
                    runtime_store(&reopened)
                        .read_worker_record(&worker.worker_ref)
                        .unwrap()
                        .restore_guard
                        .execution_termination
                        .is_none()
                );
                if retention {
                    assert!(
                        reopened
                            .execute_worker_retention(&plan(worker.worker_id))
                            .unwrap()
                            .source_removed
                    );
                } else {
                    assert!(
                        reopened
                            .delete_worker_scoped(&scope(), &worker.worker_ref)
                            .unwrap()
                            .deleted
                    );
                }
                assert_removed(&reopened, root.path(), &worker);
                assert_eq!(*backend.stop_count.lock().unwrap(), 0, "{lifecycle}");
                assert_eq!(*backend.restore_count.lock().unwrap(), 0, "{lifecycle}");
                drop(reopened);
                let reopened = Runtime::with_fs_store(options(root.path())).unwrap();
                assert_removed(&reopened, root.path(), &worker);
            }
        }
    }

    #[test]
    fn nonsettled_lifecycle_rejects_delete_retention_and_freeze_without_touching_sources() {
        for lifecycle in ["active", "create-recovery", "pending-stop"] {
            let root = tempfile::tempdir().unwrap();
            let (runtime, backend) = stored_runtime(root.path());
            let worker = create(&runtime);
            match lifecycle {
                "create-recovery" => {
                    let mut state = runtime.lock().unwrap();
                    state
                        .worker_mut(&worker.worker_ref)
                        .unwrap()
                        .require_create_recovery();
                    state.persist_worker(&worker.worker_id).unwrap();
                }
                "pending-stop" => {
                    let request =
                        test_stop_request(&runtime, &worker.worker_ref, "pending-removal-stop");
                    runtime
                        .coordinate_worker_stop(&worker.worker_ref, request)
                        .unwrap();
                }
                _ => (),
            }
            let path = worker_root(root.path(), worker.worker_id).join("worker.json");
            let before = std::fs::read(&path).unwrap();
            assert!(matches!(
                runtime.delete_worker(&worker.worker_ref),
                Err(RuntimeError::InvalidRequest(_))
            ));
            assert!(matches!(
                runtime.execute_worker_retention(&plan(worker.worker_id)),
                Err(RuntimeError::InvalidRequest(_))
            ));
            assert!(matches!(
                runtime.freeze_worker_removal_scoped(&scope(), &plan(worker.worker_id)),
                Err(RuntimeError::RecordingExecutionUnconfirmed { .. })
            ));
            assert_eq!(std::fs::read(&path).unwrap(), before, "{lifecycle}");
            assert!(!freeze_path(root.path(), worker.worker_id).exists());
            assert_eq!(*backend.stop_count.lock().unwrap(), 0);
        }
    }

    #[test]
    fn failed_stop_blocks_removal_until_a_fresh_request_confirms_termination() {
        let root = tempfile::tempdir().unwrap();
        let (runtime, backend) = stored_runtime(root.path());
        let worker = create(&runtime);
        let request = test_stop_request(&runtime, &worker.worker_ref, "cleanup-then-remove");
        // This one-shot response contains no termination evidence. The next
        // default response confirms termination only for a fresh Stop call.
        backend.set_stop_result(WorkerExecutionResult::errored(
            WorkerExecutionOperation::Stop,
            "cleanup pending",
        ));
        assert!(matches!(
            runtime.stop_worker_request(&worker.worker_ref, request.clone()),
            Err(RuntimeError::WorkerExecutionRejected { .. })
        ));
        assert_eq!(
            runtime
                .worker_stop_outcome(&worker.worker_ref, request.clone())
                .unwrap(),
            crate::catalog::WorkerStopOutcome::Failed {
                cause: RuntimeControlCause::ExecutionRejected,
            }
        );
        assert!(
            runtime
                .lock()
                .unwrap()
                .worker(&worker.worker_ref)
                .unwrap()
                .pending_stop()
                .is_none()
        );
        let failed_operation = backend.stop_operation_ids.lock().unwrap()[0];
        let path = worker_root(root.path(), worker.worker_id).join("worker.json");
        let before = std::fs::read(&path).unwrap();
        let failed = runtime_store(&runtime)
            .read_worker_record(&worker.worker_ref)
            .unwrap();
        assert!(!matches!(
            failed.execution_state,
            PersistedWorkerExecutionState::Stopped(_)
        ));
        assert!(failed.restore_guard.execution_termination.is_none());
        let failed_result = &failed.restore_guard.stop_results["cleanup-then-remove"];
        assert_eq!(
            failed_result.failure,
            Some(RuntimeControlCause::ExecutionRejected)
        );
        assert!(failed_result.receipt.is_none());
        assert!(matches!(
            runtime.delete_worker(&worker.worker_ref),
            Err(RuntimeError::InvalidRequest(_))
        ));
        assert!(matches!(
            runtime.execute_worker_retention(&plan(worker.worker_id)),
            Err(RuntimeError::InvalidRequest(_))
        ));
        assert!(matches!(
            runtime.stop_worker_request(&worker.worker_ref, request.clone()),
            Err(RuntimeError::StopRequestFailed {
                cause: RuntimeControlCause::ExecutionRejected,
                ..
            })
        ));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(*backend.stop_count.lock().unwrap(), 1);
        let fresh_request =
            test_stop_request(&runtime, &worker.worker_ref, "fresh-stop-then-remove");
        let ack = runtime
            .stop_worker_request(&worker.worker_ref, fresh_request)
            .unwrap();
        assert_eq!(ack.status, WorkerStatus::Stopped);
        assert!(!ack.replayed);
        {
            let operations = backend.stop_operation_ids.lock().unwrap();
            assert_eq!(operations.len(), 2);
            assert_eq!(operations[0], failed_operation);
            assert_ne!(operations[0], operations[1]);
        }
        assert_eq!(
            runtime
                .worker_stop_outcome(&worker.worker_ref, request)
                .unwrap(),
            crate::catalog::WorkerStopOutcome::Failed {
                cause: RuntimeControlCause::ExecutionRejected,
            }
        );
        let canonical = runtime_store(&runtime)
            .read_worker_record(&worker.worker_ref)
            .unwrap();
        assert!(matches!(
            canonical.execution_state,
            PersistedWorkerExecutionState::Stopped(_)
        ));
        assert!(canonical.restore_guard.execution_termination.is_some());
        assert!(runtime.delete_worker(&worker.worker_ref).unwrap().deleted);
        assert_eq!(
            *backend.stop_count.lock().unwrap(),
            2,
            "deletion must not stop a third time"
        );
        assert_removed(&runtime, root.path(), &worker);
    }

    #[test]
    fn unknown_terminal_stop_save_is_refreshed_before_deletion_without_restop() {
        let root = tempfile::tempdir().unwrap();
        let (runtime, backend) = stored_runtime(root.path());
        let worker = create(&runtime);
        let request = test_stop_request(&runtime, &worker.worker_ref, "unknown-save-then-remove");
        assert_eq!(
            runtime
                .coordinate_worker_stop(&worker.worker_ref, request.clone())
                .unwrap(),
            crate::catalog::WorkerStopOutcome::Pending
        );
        let operation = runtime
            .lock()
            .unwrap()
            .worker(&worker.worker_ref)
            .unwrap()
            .pending_stop()
            .unwrap()
            .operation_id;
        // Admission is already saved; dispatch reuses its in-memory Stop, so
        // AfterRename faults the terminal write after the stopped row is visible.
        runtime_store(&runtime)
            .fail_next_worker_write(crate::fs_store::AtomicWriteFault::AfterRename);
        let error = runtime
            .stop_worker_request(&worker.worker_ref, request)
            .unwrap_err();
        assert_eq!(
            error.control_cause(),
            RuntimeControlCause::StopTerminatedPersistenceOutcomeUnknown
        );
        let RuntimeError::WorkerStopPersistenceFailed {
            worker_id,
            result,
            source,
        } = &error
        else {
            panic!("expected confirmed termination with a persistence error: {error:?}");
        };
        assert_eq!(*worker_id, worker.worker_id);
        assert!(matches!(
            source.as_ref(),
            RuntimeError::StoreCommitOutcomeUnknown { .. }
        ));
        let termination = result.termination.as_ref().unwrap();
        assert!(termination.confirms_scope_shutdown(&worker.worker_ref, operation));
        assert_eq!(result.execution_id, Some(termination.execution_id));
        {
            let state = runtime.lock().unwrap();
            let record = state.worker(&worker.worker_ref).unwrap();
            assert_eq!(record.status(), WorkerStatus::Stopped);
            assert!(record.pending_stop().is_none());
            assert_eq!(
                record.restore_guard.execution_termination.as_ref(),
                Some(termination)
            );
            assert!(state.unreconciled_worker_writes.contains(&worker.worker_id));
        }
        assert!(matches!(
            runtime_store(&runtime)
                .read_worker_record(&worker.worker_ref)
                .unwrap()
                .execution_state,
            PersistedWorkerExecutionState::Stopped(_)
        ));
        assert!(runtime.delete_worker(&worker.worker_ref).unwrap().deleted);
        assert!(
            !runtime
                .lock()
                .unwrap()
                .unreconciled_worker_writes
                .contains(&worker.worker_id)
        );
        assert_eq!(*backend.stop_count.lock().unwrap(), 1);
        assert_eq!(
            backend.stop_operation_ids.lock().unwrap().as_slice(),
            &[operation]
        );
        assert_removed(&runtime, root.path(), &worker);
    }

    #[test]
    fn retention_rejects_a_changed_canonical_source_without_deleting_it() {
        let root = tempfile::tempdir().unwrap();
        let (runtime, backend) = stored_runtime(root.path());
        let worker = create(&runtime);
        runtime.stop_worker(&worker.worker_ref, None).unwrap();
        let store = runtime_store(&runtime);
        let mut saved = store.read_worker_record(&worker.worker_ref).unwrap();
        saved.restore_guard.observation_token = "different-canonical-observation".into();
        store.write_worker_record(&saved).unwrap();
        let path = worker_root(root.path(), worker.worker_id).join("worker.json");
        let before = std::fs::read(&path).unwrap();
        let error = runtime
            .execute_worker_retention(&plan(worker.worker_id))
            .unwrap_err();
        assert!(
            matches!(&error, RuntimeError::InvalidRequest(message) if message.contains("canonical source changed")),
            "{error:?}"
        );
        assert_eq!(std::fs::read(path).unwrap(), before);
        assert!(runtime.worker_detail(&worker.worker_ref).is_ok());
        assert_eq!(*backend.stop_count.lock().unwrap(), 1);
    }

    fn saved_session(
        root: &std::path::Path,
        worker: &WorkerDetail,
        receipt_submission: &str,
    ) -> session_store::SessionId {
        let session = session_store::new_session_id();
        let segment = session_store::new_segment_id();
        let owner = session_store::worker_sources::WorkerSessionSources::new(
            root,
            session_store::worker_sources::WorkerSessionOwner {
                workspace_id: scope().workspace_id,
                worker_id: worker.worker_id.to_string(),
            },
        )
        .unwrap();
        let store = session_store::WorkerSessionStore::new_with_retention(
            owner.worker_root().join("session"),
            crate::retention::runtime_session_retention(root),
        )
        .unwrap()
        .with_runtime_owner(owner.clone())
        .unwrap();
        store.create_segment(session, segment, &[
            session_store::LogEntry::AnnotatedSegmentStart {
                ts: 1, session_id: session, system_prompt: None, config: Default::default(),
                history: vec![], forked_from: None, compacted_from: None,
            },
            session_store::LogEntry::Extension {
                ts: 2,
                domain: "worker.pending_activations.v1".into(),
                payload: serde_json::json!({
                    "activating": null, "pending": [],
                    "pending_notifications": [{"message": "retained notification without Stop history"}],
                    "receipts": [{"submission_request_id": "accepted-input", "submission_id": receipt_submission}]
                }),
            },
        ]).unwrap();
        let name = crate::worker_backend::ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(
            &worker.worker_ref,
        );
        let mut metadata = session_store::WorkerMetadata::new(
            name.clone(),
            Some(session_store::WorkerActiveSegmentRef::active_segment(
                session, segment,
            )),
        );
        metadata.workspace_id = Some(scope().workspace_id);
        session_store::WorkerAggregateStore::new(owner.worker_root(), name)
            .unwrap()
            .write(&metadata)
            .unwrap();
        session
    }

    #[test]
    fn freeze_uses_optional_stop_history_to_match_inputs_and_durably_fences_ingress() {
        for history in [
            "absent",
            "no-restore-spec",
            "matching",
            "missing-receipt",
            "wrong-submission",
        ] {
            let root = tempfile::tempdir().unwrap();
            let (runtime, backend) = stored_runtime(root.path());
            let worker = create(&runtime);
            runtime.stop_worker(&worker.worker_ref, None).unwrap();
            let session = saved_session(
                root.path(),
                &worker,
                if history == "wrong-submission" {
                    "other-submission"
                } else {
                    "accepted-submission"
                },
            );
            {
                let mut state = runtime.lock().unwrap();
                let record = state.worker_mut(&worker.worker_ref).unwrap();
                if matches!(history, "absent" | "no-restore-spec") {
                    record.restore_guard.execution_termination = None;
                } else {
                    record
                        .restore_guard
                        .execution_termination
                        .as_mut()
                        .unwrap()
                        .submissions
                        .push(crate::execution::WorkerSubmissionAck {
                            submission_request_id: if history == "missing-receipt" {
                                "unrecorded-input"
                            } else {
                                "accepted-input"
                            }
                            .into(),
                            submission_id: "accepted-submission".into(),
                            disposition: protocol::SubmissionDisposition::Started,
                        });
                }
                if history == "no-restore-spec" {
                    record.request = None;
                }
                state.persist_worker(&worker.worker_id).unwrap();
            }
            drop(runtime);
            drop(backend);
            // The canonical stopped aggregate is sufficient after cold reopen;
            // there is no adapter owner whose termination could be re-queried.
            let runtime = Runtime::with_fs_store(options(root.path())).unwrap();
            runtime
                .bind_runtime_identity("trusted-removal-runtime")
                .unwrap();
            let request = plan(worker.worker_id);
            let result = runtime.freeze_worker_removal_scoped(&scope(), &request);
            if matches!(history, "missing-receipt" | "wrong-submission") {
                assert!(
                    matches!(
                        result,
                        Err(RuntimeError::RecordingExecutionUnconfirmed { .. })
                    ),
                    "{history}"
                );
                assert!(!freeze_path(root.path(), worker.worker_id).exists());
                assert!(
                    worker_root(root.path(), worker.worker_id)
                        .join("session/session.json")
                        .exists()
                );
                continue;
            }
            let frozen = result.unwrap();
            assert_eq!(frozen.members.len(), 1);
            assert_eq!(frozen.members[0].session_id, session);
            assert!(frozen.members[0].materialized);
            assert!(freeze_path(root.path(), worker.worker_id).exists());
            drop(runtime);
            let backend = Arc::new(TestExecutionBackend::default());
            let runtime =
                Runtime::with_fs_store_and_execution_backend(options(root.path()), backend.clone())
                    .unwrap();
            runtime
                .bind_runtime_identity("trusted-removal-runtime")
                .unwrap();
            assert_eq!(
                runtime
                    .freeze_worker_removal_scoped(&scope(), &request)
                    .unwrap(),
                frozen
            );
            let source = worker_root(root.path(), worker.worker_id).join("session/segments");
            let before: Vec<_> = std::fs::read_dir(&source)
                .unwrap()
                .map(|entry| {
                    let path = entry.unwrap().path();
                    (path.clone(), std::fs::read(path).unwrap())
                })
                .collect();
            assert!(
                runtime
                    .restore_worker(
                        &worker.worker_ref,
                        runtime.test_restore_request(&worker.worker_ref)
                    )
                    .is_err()
            );
            assert!(
                runtime
                    .send_input(
                        &worker.worker_ref,
                        WorkerInput::user("late input must not enter frozen source")
                    )
                    .is_err()
            );
            assert_eq!(*backend.restore_count.lock().unwrap(), 0);
            assert!(backend.dispatched_inputs.lock().unwrap().is_empty());
            for (path, bytes) in before {
                assert_eq!(std::fs::read(path).unwrap(), bytes);
            }
            assert!(
                runtime
                    .execute_worker_retention(&request)
                    .unwrap()
                    .source_removed
            );
            assert_eq!(*backend.stop_count.lock().unwrap(), 0);
            assert_removed(&runtime, root.path(), &worker);
        }
    }
}
