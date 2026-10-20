//! Restore entry -> real SQLite attachment registry -> embedded Runtime restore
//! coordination. Only execution is deterministic; Workdir providers are absent.
//! This suite covers restore preparation, not HTTP authentication or model turns.

use super::*;

#[tokio::test]
async fn restore_keeps_offline_external_and_unmaterialized_repository_attachments() {
    let temp = tempfile::tempdir().unwrap();
    init_clean_git_workspace(temp.path());
    let (api, execution) = test_api_with_recording_backend(temp.path()).await;
    execution.accept_restores();
    let Json(created) = create_workspace_worker(
        State(api.clone()),
        HeaderMap::new(),
        Json(CreateWorkspaceWorkerRequest {
            runtime_id: EMBEDDED_WORKER_RUNTIME_ID.into(),
            display_name: "Restore without providers".into(),
            singleton_key: None,
            profile: Some("builtin:coder".into()),
            ticket_assignment: None,
            initial_submit: Vec::new(),
            workdir_attachments: Vec::new(),
            feature_connections: Default::default(),
            control_operation_id: None,
            execution_grant: None,
        }),
    )
    .await
    .unwrap();
    let worker = RuntimeWorkerRef::new(&created.runtime_id, &created.worker_id);
    let grant = ExternalWorkdirGrantRecord {
        grant_id: "restore-external-grant".into(),
        workspace_id: TEST_WORKSPACE_ID.into(),
        workdir_id: "restore-external".into(),
        provider_instance_id: "offline-provider".into(),
        display_name: "Offline external".into(),
        permissions: "read_only".into(),
        created_by: test_browser_request_actor().account_id,
        created_at: TEST_CREATED_AT.into(),
        expires_at: None,
        generation: 1,
        status: "pending".into(),
        updated_at: TEST_CREATED_AT.into(),
    };
    for (alias, workdir_id, source) in [
        (
            "external",
            grant.workdir_id.as_str(),
            WorkdirRegistrySource::ExternalGrant {
                grant_id: grant.grant_id.clone(),
            },
        ),
        (
            "repository",
            "restore-missing-repository",
            WorkdirRegistrySource::Repository {
                runtime_id: EMBEDDED_WORKER_RUNTIME_ID.into(),
                repository_id: test_repository_id(&api),
            },
        ),
    ] {
        let record = WorkdirRegistryRecord {
            workspace_id: TEST_WORKSPACE_ID.into(),
            workdir_id: workdir_id.into(),
            display_name: Some(alias.into()),
            source,
            creation_selector: None,
            creation_ref: None,
            creation_tree: None,
            current_selector: None,
            current_ref: None,
            current_tree: None,
            observed_at_epoch_seconds: None,
            materialization_status: "not_found".into(),
            cleanliness: "unknown".into(),
            created_at: TEST_CREATED_AT.into(),
            updated_at: TEST_CREATED_AT.into(),
        };
        if alias == "external" {
            api.store
                .create_external_workdir_grant(&grant, &record)
                .unwrap();
            assert!(
                api.store
                    .activate_external_workdir_grant(
                        TEST_WORKSPACE_ID,
                        &grant.grant_id,
                        &grant.provider_instance_id,
                        1,
                        TEST_CREATED_AT,
                    )
                    .unwrap()
            );
        } else {
            api.store.upsert_workdir_registry(&record).unwrap();
        }
        api.store
            .attach_worker_workdir(&WorkerWorkdirLinkRecord {
                connection_id: String::new(),
                workspace_id: TEST_WORKSPACE_ID.into(),
                worker: worker.clone(),
                workdir_id: workdir_id.into(),
                alias: alias.into(),
                capabilities: workdir::WorkdirSessionCapabilities::READ_ONLY,
                linked_at: TEST_CREATED_AT.into(),
                unlinked_at: None,
            })
            .unwrap();
    }
    let links = api
        .store
        .list_worker_workdir_links(TEST_WORKSPACE_ID, &worker)
        .unwrap();
    api.runtime
        .stop_worker(
            &worker,
            WorkerLifecycleRequest {
                reason: None,
                ticket_assignment: None,
            },
        )
        .unwrap();
    let result = api
        .restore_workspace_worker(
            &worker,
            server_api::WorkerRestoreRequest {
                expected_observation_token: api
                    .runtime
                    .worker(&worker)
                    .unwrap()
                    .restore_observation_token
                    .unwrap(),
                request_id: uuid::Uuid::now_v7().to_string(),
            },
        )
        .unwrap();
    assert_eq!(result.state, server_api::WorkerRestoreState::Accepted);
    let attachments = restore_logical_workdir_attachments(&api, &worker).unwrap();
    assert_eq!(attachments.len(), 2);
    assert!(
        attachments
            .iter()
            .any(|attachment| attachment.alias.as_str() == "external"
                && attachment.working_directory_id == grant.workdir_id)
    );
    assert!(
        attachments
            .iter()
            .any(|attachment| attachment.alias.as_str() == "repository"
                && attachment.working_directory_id == "restore-missing-repository")
    );
    assert_eq!(
        api.store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, &worker)
            .unwrap(),
        links
    );
    let lock = current_worker_session_lock(&api, &worker);
    let guard = Arc::new(lock.lock_owned().await);
    for link in &links {
        let error = open_current_worker_workdir_session_locked(&api, &worker, link, &guard)
            .await
            .unwrap_err();
        match link.alias.as_str() {
            "external" => assert!(matches!(
                error,
                Error::RuntimeOperationFailed { code, .. }
                    if code == "external_workdir_provider_offline"
            )),
            "repository" => assert!(matches!(
                error,
                Error::WorkdirSessionOpenFailed { transport, .. }
                    if transport.code == workdir::http::WorkdirTransportErrorCode::Unavailable
            )),
            alias => panic!("unexpected attachment: {alias}"),
        }
    }
    assert_eq!(
        api.store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, &worker)
            .unwrap(),
        links
    );
    api.runtime
        .stop_worker(
            &worker,
            WorkerLifecycleRequest {
                reason: None,
                ticket_assignment: None,
            },
        )
        .unwrap();
}
