// Integration boundary: real Runtime, Controller/router, Git materializer and
// bare Git provider. Only model inference is replaced by the existing MockFactory.
#[test]
fn publication_fences_real_controller_workdir_and_recovers_after_worker_stop() {
    use crate::catalog::{
        LogicalWorkdirAttachment, RepositoryMaterializationContext, RepositoryPublicationBinding,
        RepositoryPublicationOutcome, RepositoryPublicationRequest,
    };
    let runtime_base = tempfile::tempdir().unwrap();
    let repo = create_clean_repo();
    git(repo.path(), &["branch", "-M", "main"]);
    let remote = tempfile::tempdir().unwrap();
    git(
        remote.path(),
        &["clone", "--bare", repo.path().to_str().unwrap(), "."],
    );
    let store = tempfile::tempdir().unwrap();
    let backend = Arc::new(
        WorkerRuntimeExecutionBackend::new(MockFactory {
            client: MockClient::new(simple_text_events()),
            runtime_base: runtime_base.path().to_path_buf(),
            cwd: repo.path().to_path_buf(),
            store_dir: store.path().join("sessions"),
            worker_metadata_dir: store.path().join("workers"),
            observed_cwds: Arc::new(Mutex::new(Vec::new())),
            observed_workspace_clients: Arc::new(Mutex::new(Vec::new())),
        })
        .unwrap()
        .with_working_directory_materializer(RuntimeGitMaterializer::new(runtime_base.path())),
    );
    let runtime =
        EmbeddedRuntime::with_execution_backend(RuntimeOptions::default(), backend.clone())
            .unwrap();
    runtime.bind_runtime_identity("runtime-test").unwrap();
    runtime.store_config_bundle(test_bundle()).unwrap();
    let alias = workdir::WorkdirAttachmentAlias::new("workdir").unwrap();
    let mut create = create_request("publication");
    create.workspace_api = Some(crate::catalog::WorkspaceApiRef {
        workspace_id: "publication-workspace".into(),
        base_url: "http://127.0.0.1:9".into(),
    });
    create.workdir_attachment_requests = vec![WorkingDirectoryAttachmentRequest {
        alias: alias.clone(),
        working_directory: working_directory_request(remote.path()),
    }];
    create.memory_settings = Some(manifest::WorkspaceMemorySettingsSnapshot {
        workspace_id: "publication-workspace".into(),
        language: "English".into(),
    });
    let detail = runtime
        .create_worker_scoped(
            &crate::runtime::RuntimeWorkspaceScope::new(
                "publication-workspace",
                "publication-test-issuer",
            ),
            create,
        )
        .unwrap();
    let id = detail.workdir_attachments[0]
        .working_directory
        .summary
        .working_directory_id
        .clone();
    runtime
        .replace_worker_workdir_attachments(
            &detail.worker_ref,
            vec![LogicalWorkdirAttachment {
                alias: alias.clone(),
                working_directory_id: id.clone(),
                capabilities: workdir::WorkdirSessionCapabilities::ALL,
            }],
        )
        .unwrap();
    let checkout = materialized_clone_root(runtime_base.path(), &id);
    let oid = |path: &std::path::Path| {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    };
    let before = oid(&checkout);
    fs::write(checkout.join("README.md"), "approved\n").unwrap();
    git(
        &checkout,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-am",
            "approved",
        ],
    );
    let source = oid(&checkout);
    let mut request = RepositoryPublicationRequest {
        binding: RepositoryPublicationBinding {
            workspace_id: "publication-workspace".into(),
            merge_request_id: "mr".into(),
            operation_id: "busy-publication".into(),
            repository_id: "repo-main".into(),
            repository_source_fingerprint: "sha256:test".into(),
            target: "refs/heads/main".into(),
            source: source.clone(),
            approval_event_id: "approval".into(),
            owner_operation_id: "owner".into(),
            worker: crate::identity::RuntimeWorkerRef::new(
                "runtime-test",
                detail.worker_ref.worker_id.to_string(),
            ),
            working_directory_id: id,
            connection_id: "connection".into(),
            alias: alias.clone(),
            target_before: before.clone(),
        },
        repository: working_directory_request(remote.path()).repository,
        materialization: RepositoryMaterializationContext {
            workspace_id: "publication-workspace".into(),
            runtime_id: "runtime-test".into(),
            operation_id: "busy-publication".into(),
            config_projection_digest: "current".into(),
            ssh: None,
        },
    };
    assert!(
        runtime
            .repository_publication_receipt(request.binding.clone())
            .unwrap()
            .is_none()
    );
    let handle = backend
        .workers
        .lock()
        .unwrap()
        .get(&detail.worker_ref)
        .unwrap()
        .handle
        .clone();
    let exclusion = backend
        .run_joined_on_adapter_runtime(async move {
            handle
                .reserve_workdir_publication(&alias)
                .await
                .map_err(|e| e.to_string())
        })
        .unwrap();
    let busy = runtime.publish_repository_source(request.clone()).unwrap();
    assert!(
        matches!(
            busy.outcome,
            RepositoryPublicationOutcome::NotPublished { .. }
        ),
        "{busy:?}"
    );
    assert_eq!(oid(remote.path()), before);
    drop(exclusion);
    assert_eq!(
        runtime.publish_repository_source(request.clone()).unwrap(),
        busy
    );
    // This is a deliberate new request, not automatic retry of the rejected one.
    request.binding.operation_id = "new-publication".into();
    request.materialization.operation_id = request.binding.operation_id.clone();
    let published = runtime.publish_repository_source(request.clone()).unwrap();
    assert!(
        matches!(
            published.outcome,
            RepositoryPublicationOutcome::Published { .. }
        ),
        "{published:?}"
    );
    assert_eq!(oid(remote.path()), source);
    runtime.stop_worker(&detail.worker_ref, None).unwrap();
    assert_eq!(
        runtime
            .reconcile_repository_publication(request.clone())
            .unwrap(),
        published
    );
    let saved_binding = request.binding.clone();
    assert_eq!(
        runtime.publish_repository_source(request).unwrap(),
        published
    );
    runtime.delete_worker(&detail.worker_ref).unwrap();
    // Neither a live Worker nor accessible Repository/checkout/config/credentials
    // can be reconstructed here. The lookup input is only the original binding.
    remote.close().unwrap();
    fs::remove_dir_all(&checkout).unwrap();
    assert_eq!(
        runtime
            .repository_publication_receipt(saved_binding.clone())
            .unwrap(),
        Some(published)
    );
    assert_eq!(
        runtime
            .repository_publication_receipt(busy.binding.clone())
            .unwrap(),
        Some(busy)
    );
    let mut wrong = saved_binding.clone();
    wrong.connection_id = "replacement".into();
    assert!(runtime.repository_publication_receipt(wrong).is_err());
    let mut wrong_runtime = saved_binding.clone();
    wrong_runtime.worker.runtime_id = "another-runtime".into();
    assert!(
        runtime
            .repository_publication_receipt(wrong_runtime)
            .is_err()
    );
    let mut absent = saved_binding;
    absent.operation_id = "never-dispatched".into();
    assert!(
        runtime
            .repository_publication_receipt(absent)
            .unwrap()
            .is_none()
    );
}
