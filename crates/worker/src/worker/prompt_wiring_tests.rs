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
    assert_eq!(state.instance.current_state.as_str(), "observe");
    assert!(
        serde_json::to_string(&state)
            .unwrap()
            .contains(&builtin.content_digest)
    );
    // Ordinary input/restoration cannot replace an active pinned definition.
    let mut restored: flow::FlowRuntimeState =
        serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
    *worker.flow_runtime_state.lock().unwrap() = Some(restored.clone());
    let (_, unchanged, _) = worker
        .prepare_flow_input(vec![Segment::Text {
            content: "Continue the existing MR from its saved Integration phase".into(),
        }])
        .unwrap();
    assert!(unchanged.is_none());
    assert!(matches!(worker.prepare_flow_input(vec![Segment::Flow {
        selector: "builtin:coder-review".into(),
    }]), Err(WorkerError::FlowInput(message)) if message.contains("active Flow")));
    assert_eq!(
        *worker.flow_runtime_state.lock().unwrap(),
        Some(restored.clone())
    );
    // Model judgments are scripted here. This checks the actual entry boundary,
    // not MR authority or whether natural-language conditions are fulfilled.
    for (n, target) in ["integrate", "complete", "done"].iter().enumerate() {
        let id = format!("resume-{n}");
        let attempt = restored
            .begin_or_recover_transition(&id, "fixture evidence")
            .unwrap();
        let results = attempt
            .transitions
            .iter()
            .map(|transition| flow::TransitionConditionResult {
                transition_id: transition.transition_id.clone(),
                verdict: if transition.target.as_str() == *target {
                    flow::ConditionVerdict::Met
                } else {
                    flow::ConditionVerdict::NotMet
                },
                rationale: "Scripted graph condition; Backend evidence is tested separately".into(),
            })
            .collect();
        restored
            .resolve_active_transition(&id, flow::FlowVerifierOutcome::Completed { results })
            .unwrap();
    }
    *worker.flow_runtime_state.lock().unwrap() = Some(restored);
    let (_, next, _) = worker
        .prepare_flow_input(vec![Segment::Flow {
            selector: "builtin:coder-review".into(),
        }])
        .unwrap();
    let next = next.unwrap();
    assert_eq!(next.instance.current_state.as_str(), "observe");
    assert_ne!(next.instance.instance_id, state.instance.instance_id);
}
