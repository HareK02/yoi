// Connect real Submit/receipt persistence, original Controller ownership and
// Runtime's canonical stop receipt. Only model I/O and one caller ACK delivery
// are substituted; no receipt, Shutdown report or joined Controller is faked.
#[cfg(feature = "fs-store")]
struct SavedSourceMockFactory(MockFactory);

#[cfg(feature = "fs-store")]
#[async_trait]
impl RuntimeWorkerFactory for SavedSourceMockFactory {
    async fn spawn_controller(
        &self,
        request: WorkerExecutionSpawnRequest,
    ) -> Result<RuntimeWorkerController, RuntimeRestoreFailure> {
        self.0
            .spawn_controller_with_saved_source(request, true)
            .await
    }

    async fn prepare_restore(
        &self,
        _: WorkerExecutionRestoreRequest,
    ) -> Result<Box<dyn RuntimeRestoreCandidate>, RuntimeRestoreFailure> {
        panic!("this fixture explicitly restores the original saved Session below")
    }
}

#[cfg(feature = "fs-store")]
#[test]
#[serial_test::serial(worker_allocation)]
fn stop_termination_lost_submit_ack_reconciles_only_original_controller_after_timeout() {
    let temp = tempfile::tempdir_in(".").unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let socket_base = PathBuf::from(temp.path().file_name().unwrap()).join("c");
    let options = crate::fs_store::FsRuntimeStoreOptions::new(root.join("catalog"))
        .with_runtime_id("lost-submit-ack");
    let client = MockClient::new(simple_text_events());
    let calls = client.call_count.clone();
    let new_backend = || {
        Arc::new(
            WorkerRuntimeExecutionBackend::new(SavedSourceMockFactory(MockFactory {
                client: client.clone(),
                runtime_base: socket_base.clone(),
                cwd: root.clone(),
                store_dir: root.join("sessions"),
                worker_metadata_dir: root.join("metadata"),
                observed_cwds: Arc::default(),
                observed_workspace_clients: Arc::default(),
            }))
            .unwrap(),
        )
    };
    let backend = new_backend();
    let runtime =
        EmbeddedRuntime::with_fs_store_and_execution_backend(options.clone(), backend.clone())
            .unwrap();
    runtime.store_config_bundle(test_bundle()).unwrap();
    let worker = runtime.create_worker(create_request("lost ACK")).unwrap();
    let original_execution = backend.workers.lock().unwrap()[&worker.worker_ref].clone();
    let mut input = WorkerInput::user("  original immutable input  ");
    input.kind = WorkerInputKind::UserIfIdle;
    input.submission_request_id = Some("original-lost-ack".into());
    *backend.lost_submission_ack.lock().unwrap() = input.submission_request_id.clone();

    // The real Controller accepts and persists; only this caller's matching ACK
    // is lost. The production acceptance timer, not a sleep, ends the wait.
    let lost = backend.dispatch_input(&worker.worker_ref, input.clone());
    assert_eq!(
        lost.outcome,
        crate::execution::WorkerExecutionOutcome::Errored
    );
    assert!(
        lost.message
            .as_deref()
            .unwrap()
            .contains("timed out waiting")
    );
    assert!(lost.submission.is_none());
    assert_eq!(
        lost.execution_id,
        Some(original_execution.tasks.execution_id)
    );
    assert!(
        original_execution
            .tasks
            .submissions
            .lock()
            .unwrap()
            .is_empty()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let (segments, source) = runtime_submission_payload(&input, "original-lost-ack");
    let durable = original_execution
        .handle
        .reconcile_submission("original-lost-ack", &segments, &source)
        .unwrap()
        .unwrap();

    let mut wrong = input.clone();
    wrong.segments = Some(vec![Segment::text("different immutable input")]);
    assert!(
        !backend
            .reconcile_submission(&worker.worker_ref, wrong)
            .is_accepted()
    );
    assert!(
        original_execution
            .tasks
            .submissions
            .lock()
            .unwrap()
            .is_empty()
    );
    let found = backend.reconcile_submission(&worker.worker_ref, input.clone());
    assert!(found.is_accepted(), "{found:?}");
    assert_eq!(
        found.execution_id,
        Some(original_execution.tasks.execution_id)
    );
    let ack = found.submission.unwrap();
    assert_eq!(ack.submission_request_id, "original-lost-ack");
    assert_eq!(ack.submission_id, durable.submission_id);
    assert_eq!(ack.disposition, durable.disposition);
    assert_eq!(
        backend
            .reconcile_submission(&worker.worker_ref, input.clone())
            .submission,
        Some(ack.clone())
    );
    assert_eq!(
        original_execution
            .tasks
            .submissions
            .lock()
            .unwrap()
            .as_slice(),
        &[ack.clone()]
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "reconcile never redispatches"
    );

    // A second real acceptance is deliberately not reconciled until after
    // shutdown. It must not amend an already-issued original termination.
    let mut late_input = WorkerInput::user("late receipt lookup");
    late_input.submission_request_id = Some("late-lost-ack".into());
    *backend.lost_submission_ack.lock().unwrap() = late_input.submission_request_id.clone();
    let late_lost = backend.dispatch_input(&worker.worker_ref, late_input.clone());
    assert!(
        late_lost
            .message
            .as_deref()
            .unwrap()
            .contains("timed out waiting")
    );
    let (late_segments, late_source) = runtime_submission_payload(&late_input, "late-lost-ack");
    let late_fingerprint =
        RuntimeExecutionTaskScope::submission_fingerprint(&late_segments, &late_source).unwrap();

    runtime.stop_worker(&worker.worker_ref, None).unwrap();
    assert!(original_execution.handle.protocol_is_closed());
    assert!(original_execution.tasks.tasks.lock().unwrap().is_empty());
    let original = runtime
        .execution_termination(&worker.worker_ref)
        .unwrap()
        .unwrap();
    assert_eq!(original.execution_id, original_execution.tasks.execution_id);
    assert_eq!(original.submissions, vec![ack.clone()]);
    assert!(original.shutdown_command_id.is_some());
    assert!(original.feature_failures.is_empty());
    let late_receipt = original_execution
        .handle
        .reconcile_submission("late-lost-ack", &late_segments, &late_source)
        .unwrap()
        .unwrap();
    let late_ack = crate::execution::WorkerSubmissionAck {
        submission_request_id: late_receipt.submission_request_id,
        submission_id: late_receipt.submission_id,
        disposition: late_receipt.disposition,
    };
    original_execution
        .tasks
        .retain_submission_acceptance(&late_ack, &late_fingerprint)
        .unwrap();
    assert_eq!(
        original_execution
            .tasks
            .termination(&worker.worker_ref, original.stop_operation_id),
        Some(original.clone()),
        "a late lookup cannot rewrite the original scope receipt"
    );
    assert_eq!(
        runtime.execution_termination(&worker.worker_ref).unwrap(),
        Some(original.clone())
    );
    drop(runtime);
    drop(backend);

    let backend = new_backend();
    let reopened =
        EmbeddedRuntime::with_fs_store_and_execution_backend(options, backend.clone()).unwrap();
    assert_eq!(
        reopened.execution_termination(&worker.worker_ref).unwrap(),
        Some(original.clone())
    );

    // Restore the same real saved Session into another actual Controller. Its
    // receipt reader must see the old ACK, but it cannot claim that acceptance
    // for this different scope, even if the caller replays the same Submit.
    let restore_root = root.clone();
    let replacement_socket = socket_base.with_file_name("c2");
    let replacement_client = MockClient::new(simple_text_events());
    let replacement_calls = replacement_client.call_count.clone();
    let replacement = backend
        .run_on_adapter_runtime(async move {
            let store = CombinedStore::new(
                FsStore::new(restore_root.join("sessions")).map_err(|error| error.to_string())?,
                FsWorkerStore::new(restore_root.join("metadata"))
                    .map_err(|error| error.to_string())?,
            );
            let fallback = WorkerManifest::from_toml(
                r#"
            [worker]
            name = "r"
            [model]
            scheme = "anthropic"
            model_id = "test-model"
            auth = { kind = "none" }
            [engine]
            max_tokens = 100
            [scope]
            allow = []
        "#,
            )
            .map_err(|error| error.to_string())?;
            let prepared =
                Worker::prepare_restored_from_worker_metadata_with_context_and_model_client(
                    "r",
                    fallback,
                    store,
                    PromptCatalogSource::builtins_only(),
                    WorkerWorkspaceContext::no_workspace(),
                    WorkerFilesystemAuthority::None,
                    Some(Box::new(replacement_client)),
                )
                .await
                .map_err(|error| error.to_string())?;
            let restored = prepared
                .activate()
                .await
                .map_err(|error| error.to_string())?;
            worker::bootstrap::start_worker_controller(
                restored,
                WorkerBootstrapLayout::Direct {
                    runtime_base: replacement_socket,
                },
            )
            .await
            .map_err(|error| error.to_string())
        })
        .unwrap();
    assert!(
        !original_execution
            .handle
            .same_controller(&replacement.handle)
    );
    let read_old = replacement
        .handle
        .reconcile_submission("original-lost-ack", &segments, &source)
        .unwrap()
        .unwrap();
    assert_eq!(read_old, durable);
    let operation = backend.worker_lock(&worker.worker_ref).unwrap();
    let connected = {
        let _guard = operation.lock().unwrap();
        backend.connect_handle(
            WorkerExecutionOperation::Restore,
            worker.worker_ref.clone(),
            test_execution_context(worker.worker_ref.clone()),
            replacement.handle,
            Arc::new(tokio::sync::Mutex::new(Some(replacement.shutdown))),
            replacement.controller_task,
            None,
            Vec::new(),
            None,
            None,
        )
    };
    assert!(
        matches!(connected, WorkerExecutionSpawnResult::Connected { .. }),
        "{connected:?}"
    );
    let replacement_execution = backend.workers.lock().unwrap()[&worker.worker_ref].clone();
    assert_ne!(
        replacement_execution.tasks.execution_id,
        original.execution_id
    );
    assert!(
        reopened
            .execution_termination(&worker.worker_ref)
            .unwrap()
            .is_none()
    );
    let read = backend.reconcile_submission(&worker.worker_ref, input.clone());
    assert!(read.is_accepted(), "{read:?}");
    assert_eq!(read.submission, Some(ack.clone()));
    assert_eq!(
        read.execution_id,
        Some(replacement_execution.tasks.execution_id)
    );
    assert!(
        replacement_execution
            .tasks
            .submissions
            .lock()
            .unwrap()
            .is_empty()
    );
    let replay = backend.dispatch_input(&worker.worker_ref, input);
    assert!(replay.is_accepted(), "{replay:?}");
    assert_eq!(replay.submission, Some(ack.clone()));
    assert!(
        replacement_execution
            .tasks
            .submission_attempts
            .lock()
            .unwrap()
            .is_empty()
    );
    assert!(
        replacement_execution
            .tasks
            .submissions
            .lock()
            .unwrap()
            .is_empty()
    );
    assert_eq!(replacement_calls.load(Ordering::SeqCst), 0);
    let mut own_input = WorkerInput::user("new Controller's own input");
    own_input.submission_request_id = Some("replacement-own-input".into());
    let own = backend.dispatch_input(&worker.worker_ref, own_input);
    assert!(own.is_accepted(), "{own:?}");
    let own_ack = own.submission.unwrap();
    assert_eq!(
        own.execution_id,
        Some(replacement_execution.tasks.execution_id)
    );
    let stopped = backend.stop_worker_operation(WorkerExecutionStopRequest {
        worker_ref: worker.worker_ref.clone(),
        operation_id: crate::execution::WorkerLifecycleOperationId::new(),
    });
    assert!(stopped.is_accepted(), "{stopped:?}");
    let replacement_receipt = stopped.termination.unwrap();
    assert_eq!(
        replacement_receipt.execution_id,
        replacement_execution.tasks.execution_id
    );
    assert_ne!(replacement_receipt.execution_id, original.execution_id);
    assert!(!replacement_receipt.submissions.contains(&ack));
    assert_eq!(replacement_receipt.submissions, vec![own_ack]);
    assert!(replacement_receipt.shutdown_command_id.is_some());
    assert!(replacement_execution.tasks.tasks.lock().unwrap().is_empty());
    assert!(
        reopened.execution_termination(&worker.worker_ref).is_err(),
        "replacement cannot overwrite original canonical receipt"
    );
}
