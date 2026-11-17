// Construction coverage only, not model-behavior measurement or a restored prompt update.
use super::*;

#[derive(Debug)]
struct GuidanceHost;
impl WorkspaceClient for GuidanceHost {
    fn workspace_id(&self) -> Option<&str> {
        Some("workspace-test")
    }
    fn kind(&self) -> &str {
        "guidance-fixture"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn authorize_feature_activation(&self, _feature: &str) -> Result<(), WorkspaceClientError> {
        // This fixture tests construction, not Backend authorization policy.
        Ok(())
    }
    fn execute(
        &self,
        request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        if request.path.ends_with("/authorization/check") {
            return Ok(WorkspaceResponse {
                status: 200,
                content_type: None,
                body: "{}".into(),
            });
        }
        assert_eq!(request.path, "/api/w/workspace-test/flows/resolve");
        let definition = flow::builtin_flow_source("coder-review")
            .unwrap()
            .compile()
            .unwrap();
        let source = flow::ResolvedFlowSource {
            selector: "builtin:coder-review".parse().unwrap(),
            workspace_id: "workspace-test".into(),
            flow_id: "guidance-fixture".into(),
            content_digest: definition.content_digest.clone(),
            definition,
        };
        Ok(WorkspaceResponse {
            status: 200,
                content_type: None,
            body: serde_json::to_string(&source).unwrap(),
        })
    }
}

#[tokio::test]
async fn builtin_coder_flow_records_resolved_instructions_and_digest() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = manifest::ProfileResolver::new()
        .with_workspace_base(dir.path())
        .resolve(
            &manifest::ProfileSelector::parse_cli("builtin:coder"),
            manifest::ProfileResolveOptions::with_worker_name("prompt-wiring"),
        )
        .unwrap()
        .manifest;
    let instruction = manifest.engine.instruction.clone();
    let cwd = dir.path().join("workspace");
    std::fs::create_dir_all(&cwd).unwrap();
    let mut worker = Worker::new(
        manifest,
        Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
        session_store::FsStore::new(dir.path().join("sessions")).unwrap(),
        WorkerWorkspaceContext::with_client(
            Some(WorkspaceId::new("workspace-test").unwrap()),
            Arc::new(GuidanceHost),
        ),
        WorkerFilesystemAuthority::local(cwd.clone(), cwd.clone()),
        Scope::writable(&cwd).unwrap(),
    )
    .await
    .unwrap();
    worker.set_system_prompt_template(
        SystemPromptTemplate::parse(
            &instruction,
            crate::prompt::source::PromptCatalogSource::builtins_only(),
        )
        .unwrap(),
    );
    worker.ensure_system_prompt_materialized().await.unwrap();
    worker.ensure_segment_head().await.unwrap();
    let report = worker.install_runtime_flow_transition_feature().unwrap();
    assert!(!report.has_errors(), "{}", report.error_message());
    let (segments, state, projection) = worker
        .prepare_flow_input(vec![Segment::Flow {
            selector: "builtin:coder-review".into(),
        }])
        .unwrap();
    let state = state.expect("new Flow");
    let builtin = flow::builtin_flow_source("coder-review")
        .unwrap()
        .compile()
        .unwrap();
    let entry = worker.projected_input_history(
        &segments,
        projection.as_ref(),
        &[SessionHistoryEntryId::new()],
        &WorkerHistoryProvenance::HumanInput {
            account_id: "fixture".into(),
        },
    );
    assert_eq!(entry.len(), 1);
    assert!(matches!(
        entry[0].annotation.origin,
        WorkerHistoryProvenance::FlowInstruction { .. }
    ));
    assert_eq!(
        entry[0].item.as_text().as_deref(),
        Some(
            builtin
                .state(&builtin.initial)
                .unwrap()
                .instructions
                .as_str()
        )
    );
    assert!(
        serde_json::to_string(&state)
            .unwrap()
            .contains(&builtin.content_digest)
    );
}
