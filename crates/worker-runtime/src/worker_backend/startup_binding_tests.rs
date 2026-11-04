mod startup_binding_tests {
    //! Exercise the factory's production binding step with a constructed Worker,
    //! real aggregate/Session stores and reopen. Only the Workspace response is
    //! synthetic in the binding cases; the direct internal Profile factory case uses
    //! a refused real loopback transport. Domain authorization is tested by Server.
    use super::*;
    use protocol::session_failure::{
        SessionPrincipal, SessionRegistrationCause, session_registration_attachment,
    };
    use protocol::startup_failure::{RuntimeStartupCause, StartupPersistenceOutcome};
    use session_store::worker_metadata::StartupFailureRead;
    use worker::{WorkspaceClientError, WorkspaceRequest, WorkspaceResponse};

    #[derive(Debug, Clone, Copy)]
    enum Response {
        Rejected,
        Lost,
        BeforeSession,
        AfterSession,
        BeforeOwner,
        AfterOwner,
        AfterAttribution,
    }
    struct AttributionClient {
        response: Response,
        store: WorkerAggregateStore,
        name: String,
    }
    impl std::fmt::Debug for AttributionClient {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("AttributionClient")
        }
    }
    impl WorkspaceClient for AttributionClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("startup-binding-workspace")
        }
        fn kind(&self) -> &str {
            "startup-binding-fixture"
        }
        fn is_available(&self) -> bool {
            true
        }
        fn execute(&self, _: WorkspaceRequest) -> Result<WorkspaceResponse, WorkspaceClientError> {
            match self.response {
                Response::AfterSession => self
                    .store
                    .update_by_name(&self.name, |metadata| {
                        metadata.active = Some(WorkerActiveSegmentRef::pending_segment(
                            session_store::new_session_id(),
                        ));
                        metadata.subjektiv_session_attribution = None;
                    })
                    .unwrap(),
                Response::AfterOwner => self
                    .store
                    .update_by_name(&self.name, |metadata| {
                        metadata.workspace_id = Some("replacement-owner".into());
                    })
                    .unwrap(),
                Response::AfterAttribution => self
                    .store
                    .update_by_name(&self.name, |metadata| {
                        metadata.subjektiv_session_attribution =
                            Some(session_store::SubjektivSessionAttributionState::Confirmed {
                                session_id: metadata.active.as_ref().unwrap().session_id,
                                subject_id: "replacement-subject".into(),
                            });
                    })
                    .unwrap(),
                _ => self.store.read_by_name(&self.name).unwrap().unwrap(),
            };
            if matches!(self.response, Response::Lost) {
                return Err(WorkspaceClientError::Request(
                    "response deliberately lost".into(),
                ));
            }
            Ok(WorkspaceResponse {
                status: 403,
                body: serde_json::to_string(
                    &server_api::RepositoryApiError::new(
                        403,
                        "PermissionDenied",
                        "synthetic rejection with private token /internal/path",
                        vec![],
                    )
                    .with_failure(
                        session_registration_attachment(SessionRegistrationCause::denied(
                            SessionPrincipal::Subject,
                        ))
                        .unwrap(),
                    ),
                )
                .unwrap(),
            })
        }
    }
    async fn check_binding(response: Response) {
        require_restore_test_environment();
        let root = tempfile::tempdir().unwrap();
        let name = "startup-binding";
        let store = WorkerAggregateStore::new(root.path(), name).unwrap();
        let session_store = WorkerSessionStore::new(root.path().join("session")).unwrap();
        let client = Arc::new(AttributionClient {
            response,
            store: store.clone(),
            name: name.into(),
        });
        let manifest = WorkerManifest::from_toml(
            r#"
            [worker]
            name = "startup-binding"
            [model]
            scheme = "anthropic"
            model_id = "unused"
            auth = { kind = "none" }
            [engine]
            max_tokens = 100
            [scope]
            allow = []
        "#,
        )
        .unwrap();
        let llm: Box<dyn LlmClient> = Box::new(MockClient::new(Vec::new()));
        let mut worker = Worker::new(
            manifest,
            Engine::<_, agen::state::Mutable, worker::SessionHistoryMetadata>::new_annotated(llm),
            CombinedStore::new(session_store, store.clone()),
            WorkerWorkspaceContext::with_client(
                Some(WorkspaceId::new("startup-binding-workspace").unwrap()),
                client,
            ),
            WorkerFilesystemAuthority::None,
            Scope::empty(),
        )
        .await
        .unwrap();
        worker.enable_worker_metadata_write_through().unwrap();
        let origin = worker.session_id();
        match response {
            Response::BeforeSession => store
                .update_by_name(name, |metadata| {
                    metadata.active = Some(WorkerActiveSegmentRef::pending_segment(
                        session_store::new_session_id(),
                    ));
                })
                .unwrap(),
            Response::BeforeOwner => store
                .update_by_name(name, |metadata| {
                    metadata.workspace_id = Some("replacement-owner".into());
                })
                .unwrap(),
            _ => store.read_by_name(name).unwrap().unwrap(),
        };
        let failure = startup_failure::finalize_startup_binding(&worker, true)
            .err()
            .expect("failure must remain a failure");
        if matches!(response, Response::Lost | Response::Rejected) {
            let mut source = std::error::Error::source(&failure);
            let mut worker = false;
            let mut host = false;
            let mut client = false;
            while let Some(current) = source {
                worker |= current.is::<WorkerError>();
                host |= current.is::<worker::subjektiv::SubjektivHostError>();
                client |= current.is::<WorkspaceClientError>();
                source = current.source();
            }
            assert!(worker && host, "common factory binding step retains in-process domain sources");
            assert_eq!(client, matches!(response, Response::Lost));
            assert!(!format!("{failure:?} {failure}").contains("response deliberately lost"));
        }
        let result = failure.execution_result(WorkerExecutionOperation::Spawn);
        if matches!(response, Response::BeforeSession | Response::BeforeOwner) {
            let primary = result.startup_failure::<RuntimeStartupCause>().unwrap();
            assert!(matches!(primary, error_contract::DecodeFailure::Known(_)));
        } else {
            let error_contract::DecodeFailure::Known(primary) = result
                .startup_failure::<SessionRegistrationCause>()
                .unwrap()
            else {
                panic!("a Session decision must not be relabeled as a generic Runtime failure");
            };
            assert_eq!(
                *primary.facts().cause(),
                if matches!(response, Response::Lost) {
                    SessionRegistrationCause::outcome_unknown()
                } else {
                    SessionRegistrationCause::denied(SessionPrincipal::Subject)
                }
            );
        }
        drop(worker);
        drop(store);
        let reopened = WorkerAggregateStore::new(root.path(), name).unwrap();
        let metadata = reopened.read_by_name(name).unwrap().unwrap();
        match response {
            Response::Rejected | Response::Lost => {
                assert_eq!(
                    result.startup_failure_persistence,
                    Some(StartupPersistenceOutcome::Persisted)
                );
                let StartupFailureRead::Present(error_contract::DecodeFailure::Known(cause)) =
                    metadata.decode_startup_failure::<SessionRegistrationCause>()
                else {
                    panic!("same-Session failure must actually save a typed cause")
                };
                assert_eq!(
                    *cause.facts().cause(),
                    if matches!(response, Response::Lost) {
                        SessionRegistrationCause::outcome_unknown()
                    } else {
                        SessionRegistrationCause::denied(SessionPrincipal::Subject)
                    }
                );
                assert_eq!(metadata.active.unwrap().session_id, origin);
                let saved = serde_json::to_string(&metadata.startup_failure).unwrap();
                assert!(!saved.contains("private token"));
                assert!(!saved.contains("/internal/path"));
                assert_eq!(
                    metadata
                        .subjektiv_session_attribution
                        .as_ref()
                        .unwrap()
                        .session_id(),
                    origin
                );
                assert_eq!(
                    matches!(
                        metadata.subjektiv_session_attribution,
                        Some(
                            session_store::SubjektivSessionAttributionState::OutcomeUnknown { .. }
                        )
                    ),
                    matches!(response, Response::Lost)
                );
            }
            _ => {
                assert!(
                    matches!(
                        metadata.decode_startup_failure::<RuntimeStartupCause>(),
                        StartupFailureRead::Missing
                    ),
                    "replacement Session/owner must not acquire stale cause"
                );
                assert_ne!(
                    result.startup_failure_persistence,
                    Some(StartupPersistenceOutcome::Persisted)
                );
                if matches!(response, Response::BeforeSession | Response::AfterSession) {
                    assert_ne!(metadata.active.unwrap().session_id, origin);
                }
                if matches!(
                    response,
                    Response::BeforeOwner | Response::BeforeSession | Response::AfterSession
                ) {
                    assert!(metadata.subjektiv_session_attribution.is_none());
                }
                if matches!(response, Response::AfterOwner) {
                    assert!(matches!(
                        metadata.subjektiv_session_attribution,
                        Some(
                            session_store::SubjektivSessionAttributionState::OutcomeUnknown { .. }
                        )
                    ));
                }
                if matches!(response, Response::BeforeOwner | Response::AfterOwner) {
                    assert_eq!(metadata.workspace_id.as_deref(), Some("replacement-owner"));
                }
                if matches!(response, Response::AfterAttribution) {
                    assert!(matches!(
                        metadata.subjektiv_session_attribution,
                        Some(session_store::SubjektivSessionAttributionState::Confirmed { .. })
                    ));
                }
            }
        }
    }
    #[cfg(feature = "http-server")]
    #[tokio::test]
    #[serial_test::serial(worker_allocation)]
    async fn internal_profile_factory_transport_failure_retains_source_chain_and_reopened_cause() {
        require_restore_test_environment();
        let root = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let worker_ref = WorkerRef::new(WorkerId::now_v7());
        let mut request = create_request("internal factory source");
        request.worker_id = worker_ref.worker_id;
        request.workspace_api = Some(WorkspaceApiRef { workspace_id: "adapter-test".into(), base_url: endpoint.clone() });
        request.subjektiv_attached = true;
        request.memory_settings = Some(manifest::WorkspaceMemorySettingsSnapshot { workspace_id: "adapter-test".into(), language: "English".into() });
        let archive = crate::profile_archive::ProfileSourceArchive::build_evaluated_profile(
            "profile-source-archive:internal-source".into(), "builtin:companion".into(),
            serde_json::json!({"slug":"internal-source", "description":"internal source test", "scope":"workspace_read",
                "model":{"scheme":"anthropic", "model_id":"unused", "auth":{"kind":"none"}},
                "engine":{"max_tokens":100}, "feature":{"subjektiv":{"enabled":true,"extraction":{"enabled":false}}}})
        ).unwrap();
        let mut bundle = test_bundle();
        bundle.profile_source_archive = Some(archive.clone());
        let bundle = bundle.with_computed_digest();
        request.profile_source = crate::catalog::ProfileSourceArchiveSource::Embedded { archive };
        request.config_bundle = Some(ConfigBundleRef { id: bundle.metadata.id.clone(), digest: bundle.metadata.digest.clone() });
        let request = WorkerExecutionSpawnRequest {
            worker_ref: worker_ref.clone(), request,
            workspace_scope: None,
            context: test_execution_context(worker_ref.clone()),
            workdir_attachments: BTreeMap::new(), config_bundle: Some(bundle),
        };
        // Direct internal caller, not a Browser handler or Runtime result decoder.
        // The real transport is refused at an unused loopback socket; only the model is replaced.
        let factory = ProfileRuntimeWorkerFactory::new(root.path())
            .with_runtime_id("internal-source-runtime")
            .with_runtime_store_dir(root.path().join("runtime"))
            .with_spawn_model_client_factory(|| Box::new(MockClient::new(vec![])));
        let failure = match factory.spawn_controller(request).await {
            Err(failure) => failure,
            Ok(_) => panic!("disconnected Backend must not create a Controller"),
        };
        assert_eq!(failure.stage, RuntimeRestoreStage::Binding);
        let mut source = std::error::Error::source(&failure);
        let mut worker = false; let mut host = false; let mut transport = false;
        while let Some(current) = source {
            worker |= current.is::<WorkerError>();
            host |= current.is::<worker::subjektiv::SubjektivHostError>();
            if let Some(error) = current.downcast_ref::<WorkspaceClientError>() {
                assert!(matches!(error, WorkspaceClientError::Request(_)));
                transport = true;
            }
            source = current.source();
        }
        assert!(worker && host && transport, "factory must retain original typed in-process source");
        let result = failure.execution_result(WorkerExecutionOperation::Spawn);
        let Some(error_contract::DecodeFailure::Known(cause)) = result.startup_failure::<SessionRegistrationCause>() else { panic!("Session cause must remain typed") };
        assert_eq!(*cause.facts().cause(), SessionRegistrationCause::outcome_unknown());
        assert_eq!(result.startup_failure_persistence, Some(StartupPersistenceOutcome::Persisted));
        let aggregate = root.path().join("runtime/workers").join(worker_ref.worker_id.to_string());
        let name = ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(&worker_ref);
        let metadata = WorkerAggregateStore::new(aggregate, &name).unwrap().read_by_name(&name).unwrap().unwrap();
        let StartupFailureRead::Present(error_contract::DecodeFailure::Known(cause)) = metadata.decode_startup_failure::<SessionRegistrationCause>() else { panic!("saved factory cause must reopen") };
        assert_eq!(*cause.facts().cause(), SessionRegistrationCause::outcome_unknown());
        assert!(!format!("{failure:?} {failure}").contains(&endpoint));
        assert!(!serde_json::to_string(&result).unwrap().contains(&endpoint));
    }

    #[tokio::test]
    async fn factory_rejection_saves_same_session_cause_after_own_attribution_transition() {
        check_binding(Response::Rejected).await;
    }
    #[tokio::test]
    async fn factory_unknown_response_saves_cause_without_claiming_attribution_success() {
        check_binding(Response::Lost).await;
    }
    #[tokio::test]
    async fn factory_session_replaced_before_capture_rejects_stale_cause() {
        check_binding(Response::BeforeSession).await;
    }
    #[tokio::test]
    async fn factory_session_replaced_after_capture_rejects_stale_cause() {
        check_binding(Response::AfterSession).await;
    }
    #[tokio::test]
    async fn factory_owner_replaced_before_capture_rejects_stale_cause() {
        check_binding(Response::BeforeOwner).await;
    }
    #[tokio::test]
    async fn factory_owner_replaced_after_capture_rejects_stale_cause() {
        check_binding(Response::AfterOwner).await;
    }
    #[tokio::test]
    async fn factory_attribution_replaced_after_capture_preserves_new_attribution() {
        check_binding(Response::AfterAttribution).await;
    }
}
