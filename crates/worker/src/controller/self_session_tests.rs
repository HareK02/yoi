// Real Worker/Controller composition and FsStore; only the external model and
// Workspace transport are substituted. A self read runs inside the active turn.
#[tokio::test]
async fn self_read_during_run_uses_local_session_without_workspace_access() {
    for (workspace, self_read) in [
        (None, true),
        (Some("workspace-1"), true),
        (Some("workspace-1"), false),
    ] {
        let root = TempDir::new().unwrap();
        let mut manifest = manifest::WorkerManifest::from_toml(
            "[worker]\nname = 'self-observation'\n[model]\nscheme = 'anthropic'\nmodel_id = 'test'\n[engine]\n[scope]\n"
        ).unwrap();
        manifest.feature.worker.enabled = true;
        manifest.feature.worker.self_read = self_read;
        manifest.feature.sub_worker.enabled = false;
        manifest.feature.workdir_catalog.enabled = false;
        let client = ScriptedClient::new(vec![
            vec![
                LlmEvent::tool_use_start(0, "read-self", "SearchSessionEntries"),
                LlmEvent::tool_input_delta(
                    0,
                    r#"{"target":{"kind":"self"},"query":"local-evidence-token","kind":"user"}"#,
                ),
                LlmEvent::tool_use_stop(0),
            ],
            finished_response(),
        ]);
        let requests = client.requests.clone();
        let engine =
            Engine::<_, agen::state::Mutable, crate::SessionHistoryMetadata>::new_annotated(client);
        let store = session_store::CombinedStore::new(
            session_store::FsStore::new(root.path().join("sessions")).unwrap(),
            session_store::FsWorkerStore::new(root.path().join("workers")).unwrap(),
        );
        let operations = Arc::new(std::sync::Mutex::new(Vec::new()));
        let host = Arc::new(DriveWorkspaceHost {
            available: workspace.is_some(),
            workspace,
            granted: false,
            operations: operations.clone(),
        });
        let mut worker = Worker::new(
            manifest,
            engine,
            store,
            crate::worker::WorkerWorkspaceContext::with_client(
                workspace.map(|id| crate::worker::WorkspaceId::new(id).unwrap()),
                host,
            ),
            crate::worker::WorkerFilesystemAuthority::None,
            manifest::Scope::empty(),
        )
        .await
        .unwrap();
        worker.set_system_prompt_template(
            crate::SystemPromptTemplate::parse(
                "default",
                crate::PromptCatalogSource::builtins_only(),
            )
            .unwrap(),
        );
        let mut initialized = WorkerController::initialize(worker, root.path(), None)
            .await
            .unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            initialized
                .worker
                .run_text("local-evidence-token: verify this saved input"),
        )
        .await
        .expect("self capture must not wait on its own Controller")
        .unwrap();
        assert!(
            operations.lock().unwrap().is_empty(),
            "self access must not authorize, discover or capture through Workspace"
        );
        let requests = requests.lock().unwrap();
        let result = requests
            .last()
            .unwrap()
            .items
            .iter()
            .find(|item| matches!(item, Item::ToolResult { call_id, .. } if call_id == "read-self"))
            .expect("self observation result returned to model");
        match result {
            Item::ToolResult {
                content,
                is_error,
                summary,
                ..
            } => {
                assert_eq!(*is_error, !self_read, "{result:?}");
                if self_read {
                    assert!(content.as_deref().unwrap().contains("local-evidence-token"));
                } else {
                    assert!(format!("{summary} {content:?}").contains("disabled"));
                }
            }
            _ => unreachable!(),
        }
    }
}
