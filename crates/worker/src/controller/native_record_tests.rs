// Controller composition regression: real registration and model-facing Tools, with only
// Workspace transport substituted. MR-only publication must not depend on Ticket activation.
#[tokio::test]
async fn mr_only_controller_reserves_ticket_coordinates_without_publishing_ticket_or_granting_access()
 {
    use crate::worker::{
        WorkspaceClient, WorkspaceClientError, WorkspaceRequest, WorkspaceResponse,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    #[derive(Debug)]
    struct Host {
        allowed: AtomicBool,
        requests: Mutex<Vec<WorkspaceRequest>>,
    }
    impl WorkspaceClient for Host {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace")
        }
        fn kind(&self) -> &str {
            "mr-only-controller"
        }
        fn is_available(&self) -> bool {
            true
        }
        fn execute(
            &self,
            request: WorkspaceRequest,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            if request.path.ends_with("/authorization/check") {
                let check: server_api::authorization::OperationAuthorizationRequest =
                    serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
                assert_eq!(check.operation, "merge_request.open");
                assert_eq!(
                    check.resource,
                    server_api::authorization::PermissionResource::Ticket {
                        workspace_id: "workspace".into(),
                        ticket_id: "T-42".into()
                    }
                );
                return Ok(WorkspaceResponse {
                    status: if self.allowed.load(Ordering::SeqCst) {
                        200
                    } else {
                        403
                    },
                    body: "{}".into(),
                });
            }
            assert_eq!(request.path, "/api/w/workspace/tickets/T-42/merge-request");
            self.requests.lock().unwrap().push(request);
            Ok(WorkspaceResponse {
                status: 200,
                body: serde_json::json!({"merge_request_id":"MR-42"}).to_string(),
            })
        }
    }
    let root = TempDir::new().unwrap();
    let mut manifest=manifest::WorkerManifest::from_toml("[worker]\nname = 'mr-only'\n[model]\nscheme = 'anthropic'\nmodel_id = 'test'\n[engine]\n[scope]\n").unwrap();
    manifest.worker.mode = manifest::WorkerMode::Wip;
    manifest.feature.ticket.enabled = false;
    manifest.feature.objective.enabled = false;
    manifest.feature.drive.enabled = false;
    manifest.feature.workdir_catalog.enabled = false;
    manifest.feature.merge_request = manifest::MergeRequestFeatureConfig {
        open: true,
        ..Default::default()
    };
    let host = Arc::new(Host {
        allowed: AtomicBool::new(true),
        requests: Mutex::new(vec![]),
    });
    let engine = Engine::<_, agen::state::Mutable, crate::SessionHistoryMetadata>::new_annotated(
        ScriptedClient::new(vec![]),
    );
    let store = session_store::CombinedStore::new(
        session_store::FsStore::new(root.path().join("sessions")).unwrap(),
        session_store::FsWorkerStore::new(root.path().join("workers")).unwrap(),
    );
    let context = crate::worker::WorkerWorkspaceContext::with_client(
        Some(crate::worker::WorkspaceId::new("workspace").unwrap()),
        host.clone(),
    );
    let mut worker = Worker::new(
        manifest,
        engine,
        store,
        context,
        crate::worker::WorkerFilesystemAuthority::None,
        manifest::Scope::empty(),
    )
    .await
    .unwrap();
    register_worker_tools(
        &mut worker,
        root.path().join("runtime"),
        SpawnedWorkerRegistry::new_for_internal_services(),
        None,
        None,
    )
    .await
    .unwrap();
    assert!(
        host.requests.lock().unwrap().is_empty(),
        "registration does not authorize or dispatch domain work"
    );
    let tools = worker.engine().tool_server_handle();
    tools.flush_pending();
    let (_, inspect) = tools.get_tool("Inspect").unwrap();
    assert!(
        inspect
            .execute(
                r#"{"path":"/tickets/T-42"}"#,
                ToolExecutionContext::direct()
            )
            .await
            .is_err()
    );
    let output = inspect
        .execute(
            r#"{"path":"/merge-requests"}"#,
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
    let object: serde_json::Value = serde_json::from_str(&output.content.unwrap()).unwrap();
    let definition = inspect
        .execute(
            &serde_json::json!({"path":object["interfaces"][0]["path"]}).to_string(),
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
    assert!(definition.content.unwrap().contains("ticket: entry"));
    let (_, invoke) = tools.get_tool("Invoke").unwrap();
    let input = serde_json::json!({"path":"/merge-requests","interface":object["interfaces"][0]["reference"],"operation":"open","arguments":{"ticket":"/tickets/T-42","repository":{"key":"main","source":"work/T-42","target":"develop"}}});
    invoke
        .execute(&input.to_string(), ToolExecutionContext::direct())
        .await
        .unwrap();
    assert_eq!(host.requests.lock().unwrap().len(), 1);
    host.allowed.store(false, Ordering::SeqCst);
    assert!(
        invoke
            .execute(&input.to_string(), ToolExecutionContext::direct())
            .await
            .is_err()
    );
    assert_eq!(
        host.requests.lock().unwrap().len(),
        1,
        "revocation prevents a second domain effect"
    );
    for target in [
        "T-42",
        "/objectives/O-42",
        "/tickets-extra/T-42",
        "/tickets/T-42/child",
    ] {
        let mut invalid = input.clone();
        invalid["arguments"]["ticket"] = serde_json::json!(target);
        assert!(
            invoke
                .execute(&invalid.to_string(), ToolExecutionContext::direct())
                .await
                .is_err()
        );
    }
    assert_eq!(host.requests.lock().unwrap().len(), 1);
    assert!(tools.get_tool("TicketCreate").is_none());
    assert!(
        inspect
            .execute(
                r#"{"path":"/tickets/T-42"}"#,
                ToolExecutionContext::direct()
            )
            .await
            .is_err()
    );
}
