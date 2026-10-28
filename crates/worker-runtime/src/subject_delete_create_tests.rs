// Actual create factory future remains behind Runtime's Worker operation lock;
// deletion must wait for that owner, then permanently reject the delayed retry.
struct SubjectDeleteDeferredCreate {
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
        _: WorkerExecutionSpawnRequest,
    ) -> Result<RuntimeWorkerController, String> {
        self.entered.send(()).unwrap();
        self.release.notified().await;
        Err("create failed before any Controller".into())
    }
}

#[test]
fn subject_delete_serializes_inflight_create_and_rejects_all_late_admission() {
    let root = tempfile::tempdir().unwrap();
    let (entered, entry) = mpsc::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let backend = Arc::new(
        WorkerRuntimeExecutionBackend::new(SubjectDeleteDeferredCreate {
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
    assert!(creating.join().unwrap().is_err());
    deleting.join().unwrap().unwrap();
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
