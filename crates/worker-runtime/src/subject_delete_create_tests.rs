// Actual create factory future remains behind Runtime's Worker operation lock;
// deletion must wait for that owner, then permanently reject the delayed retry.
struct SubjectDeleteDeferredCreate {
    inner: Option<MockFactory>,
    handles: Arc<Mutex<Vec<WorkerHandle>>>,
    entered: mpsc::Sender<()>,
    release: Arc<tokio::sync::Notify>,
}
#[async_trait]
impl RuntimeWorkerFactory for SubjectDeleteDeferredCreate {
    async fn prepare_restore(
        &self,
        _: WorkerExecutionRestoreRequest,
    ) -> Result<Box<dyn RuntimeRestoreCandidate>, RuntimeRestoreFailure> {
        panic!("fenced create must never restore")
    }
    async fn spawn_controller(
        &self,
        request: WorkerExecutionSpawnRequest,
    ) -> Result<RuntimeWorkerController, RuntimeRestoreFailure> {
        self.entered.send(()).unwrap();
        self.release.notified().await;
        if let Some(inner) = &self.inner {
            let controller = inner.spawn_controller(request).await?;
            self.handles.lock().unwrap().push(controller.handle.clone());
            Ok(controller)
        } else {
            Err("create failed before any Controller".into())
        }
    }
}

#[test]
fn subject_delete_serializes_inflight_create_and_rejects_all_late_admission() {
    subject_delete_inflight_create(true);
}

#[test]
fn subject_delete_unproven_create_failure_remains_fenced_without_termination() {
    subject_delete_inflight_create(false);
}

fn subject_delete_inflight_create(real_controller: bool) {
    let root = tempfile::tempdir().unwrap();
    let (entered, entry) = mpsc::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let handles = Arc::new(Mutex::new(Vec::new()));
    let inner = real_controller.then(|| MockFactory {
        client: MockClient::new(vec![]),
        runtime_base: root.path().join("control"),
        cwd: root.path().to_path_buf(),
        store_dir: root.path().join("sessions"),
        worker_metadata_dir: root.path().join("metadata"),
        observed_cwds: Arc::default(),
        observed_workspace_clients: Arc::default(),
    });
    let backend = Arc::new(
        WorkerRuntimeExecutionBackend::new(SubjectDeleteDeferredCreate {
            inner,
            handles: handles.clone(),
            entered,
            release: release.clone(),
        })
        .unwrap(),
    );
    let runtime = EmbeddedRuntime::with_fs_store_and_execution_backend(
        crate::fs_store::FsRuntimeStoreOptions::new(root.path()).with_runtime_id("delete-test"),
        backend.clone(),
    )
    .unwrap();
    runtime.store_config_bundle(test_bundle()).unwrap();
    let scope = crate::runtime::RuntimeWorkspaceScope::new("ws-test", "server-test");
    let mut create = create_request("inflight create");
    create.workspace_api = Some(WorkspaceApiRef {
        workspace_id: scope.workspace_id.clone(),
        base_url: "https://workspace.invalid".into(),
    });
    create.memory_settings = Some(manifest::WorkspaceMemorySettingsSnapshot {
        workspace_id: scope.workspace_id.clone(),
        language: "English".into(),
    });
    let creating = {
        let runtime = runtime.clone();
        let scope = scope.clone();
        let create = create.clone();
        std::thread::spawn(move || runtime.create_worker_scoped(&scope, create))
    };
    entry.recv_timeout(Duration::from_secs(5)).unwrap();
    let request = runtime_api::SubjectDeleteWorkerRequest {
        workspace_id: scope.workspace_id.clone(),
        worker_id: create.worker_id,
        create_fingerprint: create.create_fingerprint.clone(),
        operation_id: "delete-creating".into(),
    };
    let deleting = {
        let runtime = runtime.clone();
        let scope = scope.clone();
        let request = request.clone();
        std::thread::spawn(move || runtime.subject_delete_worker_scoped(&scope, request))
    };
    release.notify_one();
    let creation = creating.join().unwrap();
    let deletion = deleting.join().unwrap();
    let worker_ref = WorkerRef::new(create.worker_id);
    if real_controller {
        creation.unwrap();
        deletion.unwrap();
        let original = runtime.execution_termination(&worker_ref).unwrap().unwrap();
        assert_eq!(
            original.scope,
            crate::execution::WorkerExecutionTerminationScope::Controller
        );
        assert!(original.shutdown_command_id.is_some());
        assert!(original.feature_failures.is_empty());
        assert_eq!(
            backend.admissions.lock().unwrap()[&worker_ref].deletion_stop_operation_id,
            Some(original.stop_operation_id)
        );
        assert_eq!(
            backend.termination_receipt(&worker_ref).unwrap(),
            Some(original)
        );
        assert_eq!(handles.lock().unwrap().len(), 1);
        assert!(handles.lock().unwrap()[0].protocol_is_closed());
    } else {
        assert!(creation.is_err());
        assert!(deletion.is_err());
        assert!(
            runtime
                .execution_termination(&worker_ref)
                .unwrap()
                .is_none()
        );
        assert!(backend.completed_stops.lock().unwrap().is_empty());
        assert!(
            backend
                .pending_spawns
                .lock()
                .unwrap()
                .contains_key(&worker_ref)
        );
        assert!(handles.lock().unwrap().is_empty());
        assert!(runtime.worker_detail(&worker_ref).is_ok());
    }
    assert!(
        runtime
            .create_worker_scoped(&scope, create.clone())
            .is_err()
    );
    assert!(backend.workers.lock().unwrap().is_empty());
    let worker_ref = WorkerRef::new(create.worker_id);
    let spawn = WorkerExecutionSpawnRequest {
        worker_ref: worker_ref.clone(),
        request: create.clone(),
        workspace_scope: Some(scope.clone()),
        context: test_execution_context(worker_ref.clone()),
        workdir_attachments: BTreeMap::new(),
        config_bundle: None,
    };
    assert!(matches!(
        backend.spawn_worker(spawn),
        WorkerExecutionSpawnResult::Rejected(_)
    ));
    let mut restore = direct_restore_request(worker_ref);
    restore.request = create;
    restore.workspace_scope = Some(scope);
    assert!(matches!(
        backend.restore_worker(restore),
        WorkerExecutionSpawnResult::Rejected(_)
    ));
}
