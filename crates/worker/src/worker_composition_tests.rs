// Production bootstrap, Controller registration, immutable manifest storage and
// restore. Model and Workspace transport are fixtures; no live external policy
// outcome is claimed. All cases have no Workdir attachment or delegation grant.
use super::*;

fn bootstrap(
    temp: &tempfile::TempDir,
    manifest: WorkerManifest,
    store: BodyPublicationStore,
    client: Option<Arc<FeatureAdmissionClient>>,
) -> crate::WorkerBootstrap<BodyPublicationStore> {
    let context = client.map_or_else(WorkerWorkspaceContext::no_workspace, |client| {
        WorkerWorkspaceContext::with_client(
            Some(WorkspaceId::new("feature-restore-workspace").unwrap()),
            client,
        )
    });
    crate::WorkerBootstrap::new(
        manifest,
        store,
        PromptCatalogSource::builtins_only(),
        context,
        WorkerFilesystemAuthority::None,
        crate::WorkerBootstrapLayout::RuntimeManagedRun {
            run_dir: temp
                .path()
                .join("runtime/runs")
                .join(uuid::Uuid::now_v7().to_string()),
        },
    )
    .with_model_client(NoopClient)
}

fn assert_publication(
    controller: &crate::BootstrappedWorker,
    objective: bool,
    sub_worker: bool,
    mode: manifest::WorkerMode,
) {
    let names = controller.handle.shared_state.greeting().tools;
    if mode == manifest::WorkerMode::Tools {
        for tool in ["Read", "Write", "Edit", "Glob", "Grep", "Bash"] {
            assert!(
                names.iter().any(|name| name == tool),
                "empty attachment must retain {tool}: {names:?}"
            );
        }
        assert_eq!(
            names
                .iter()
                .filter(|name| name.as_str() == "QueryObjective")
                .count(),
            usize::from(objective)
        );
        assert_eq!(
            names
                .iter()
                .filter(|name| name.as_str() == "SubWorkerSpawn")
                .count(),
            usize::from(sub_worker)
        );
    } else {
        assert_eq!(names, ["Inspect", "Invoke", "Tree"]);
    }
    let state = session_store::collect_state(&controller.handle.committed_entries());
    let prompt = state.system_prompt.unwrap();
    assert_eq!(prompt.contains("## SubWorker orchestration"), sub_worker);
}

#[tokio::test]
async fn async_chat_profile_prunes_execution_tools_without_granting_authority() {
    let temp = tempfile::tempdir().unwrap();
    let _sandbox = worker_allocation::test_util::RuntimeDirSandbox::new(temp.path());
    let resolved = manifest::ProfileResolver::new()
        .with_workspace_base(temp.path())
        .resolve(
            &manifest::ProfileSelector::source_named(
                manifest::ProfileRegistrySource::Builtin,
                "async-chat",
            ),
            manifest::ProfileResolveOptions::with_worker_name("chat-composition"),
        )
        .unwrap();
    let mut manifest = resolved.manifest;
    // This case isolates registration restrictions from independently tested
    // Subject startup Memory/Journal authorization and chat Host requirements.
    manifest.feature.subjektiv.profile.enabled = false;
    let (_, store) = feature_restore_fixture(&temp);
    let client = Arc::new(FeatureAdmissionClient::default());
    let controller = bootstrap(&temp, manifest, store, Some(client.clone()))
        .prepare()
        .await
        .unwrap()
        .start()
        .await
        .unwrap();
    let names = controller.handle.shared_state.greeting().tools;
    for forbidden in [
        "Read",
        "Write",
        "Edit",
        "Glob",
        "Grep",
        "Bash",
        "SubWorkerSpawn",
        "WebFetch",
        "WebSearch",
        "SpawnTicketWorker",
        "JobRun",
    ] {
        assert!(
            !names.iter().any(|name| name == forbidden),
            "chat body exposed {forbidden}: {names:?}"
        );
    }
    for required in [
        "WorkerSpawn",
        "WorkerNotify",
        "WorkerStop",
        "WorkerList",
        "QueryObjective",
        "ShowTicket",
    ] {
        assert!(
            names.iter().any(|name| name == required),
            "chat body lacks {required}: {names:?}"
        );
    }
    assert!(
        client.requests.lock().unwrap().is_empty(),
        "Profile activation must not issue authority or read protected content"
    );
    stop_body_publication_controller(controller).await;
}

#[tokio::test]
async fn objective_and_sub_worker_saved_composition_survives_create_and_restore_matrix() {
    for mode in [manifest::WorkerMode::Tools, manifest::WorkerMode::Wip] {
        for workspace in [false, true] {
            for objective in [false, true] {
                for sub_worker in [false, true] {
                    let temp = tempfile::tempdir().unwrap();
                    let _sandbox =
                        worker_allocation::test_util::RuntimeDirSandbox::new(temp.path());
                    let (mut manifest, store) = feature_restore_fixture(&temp);
                    manifest.worker.mode = mode;
                    manifest.feature.ticket.enabled = false;
                    manifest.feature.workdir_catalog.enabled = false;
                    manifest.feature.objective.enabled = objective;
                    manifest.feature.sub_worker.enabled = sub_worker;
                    let client = Arc::new(FeatureAdmissionClient::default());
                    // Denied operation permissions must not prevent local publication.
                    *client.response_status.lock().unwrap() = Some(403);
                    let prepared = bootstrap(
                        &temp,
                        manifest.clone(),
                        store.clone(),
                        workspace.then(|| client.clone()),
                    )
                    .prepare()
                    .await
                    .unwrap();
                    assert!(
                        prepared
                            .worker()
                            .workdir_sessions()
                            .only_session()
                            .is_none()
                    );
                    assert!(
                        prepared
                            .worker()
                            .manifest()
                            .delegation_scope
                            .allow
                            .is_empty()
                    );
                    let created = prepared.start().await;
                    let rejected = objective && !workspace;
                    if rejected {
                        assert!(
                            format!("{:?}", created.err().unwrap()).contains(
                                "objective tools require Backend Workspace API authority"
                            )
                        );
                    } else {
                        let controller = created.unwrap();
                        assert_publication(&controller, objective, sub_worker, mode);
                        stop_body_publication_controller(controller).await;
                    }
                    let mut fallback = manifest.clone();
                    fallback.feature.objective.enabled = !objective;
                    fallback.feature.sub_worker.enabled = !sub_worker;
                    fallback.worker.mode = if mode == manifest::WorkerMode::Tools {
                        manifest::WorkerMode::Wip
                    } else {
                        manifest::WorkerMode::Tools
                    };
                    let restored = bootstrap(
                        &temp,
                        fallback,
                        store.clone(),
                        workspace.then(|| client.clone()),
                    )
                    .prepare_restored(&manifest.worker.name)
                    .await
                    .unwrap();
                    assert_eq!(restored.worker().manifest().feature, manifest.feature);
                    assert_eq!(restored.worker().manifest().worker.mode, mode);
                    let restored = restored.start().await;
                    if rejected {
                        assert!(
                            format!("{:?}", restored.err().unwrap()).contains(
                                "objective tools require Backend Workspace API authority"
                            )
                        );
                    } else {
                        let controller = restored.unwrap();
                        assert_publication(&controller, objective, sub_worker, mode);
                        stop_body_publication_controller(controller).await;
                    }
                    assert!(
                        client.requests.lock().unwrap().is_empty(),
                        "composition never requests Feature activation or protected data"
                    );
                }
            }
        }
    }
}
