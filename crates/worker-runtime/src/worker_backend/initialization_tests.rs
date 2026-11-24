mod initialization_tests {
    //! Production Runtime create/FS persistence, Profile factory, Controller and
    //! same-ID restore. Only the model and an additional Feature provider are
    //! substitutes. Run with fs-store + ws-server and the normal isolated Worker
    //! allocation test environment; this is not Backend domain authorization.
    use super::*;
    use protocol::feature_initialization::{FeatureInitialization, FeatureInitializations};
    use worker::feature::FeatureDescriptor;
    use worker::feature::initialization::{
        FeatureInitializationAdapter, InitializationContext, InitializationPhase,
    };

    const FEATURE: &str = "builtin:test-initialization";
    type TestWorker = Worker<Box<dyn LlmClient>, ProfileRestoreStore>;

    #[derive(serde::Serialize, serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct LaunchValue {
        tokens: u32,
    }

    #[derive(Clone, Default)]
    struct Probe {
        bound: Arc<AtomicUsize>,
        initialized: Arc<AtomicBool>,
        effects: Arc<Mutex<Vec<(session_store::SessionId, InitializationPhase)>>>,
    }
    impl Probe {
        fn decode(data: &FeatureInitializations) -> Result<LaunchValue, String> {
            data.get(FEATURE)
                .ok_or("test Feature payload missing")?
                .decode(FEATURE, 1)
        }
    }
    impl FeatureInitializationAdapter<Box<dyn LlmClient>, ProfileRestoreStore> for Probe {
        fn descriptor(&self) -> FeatureDescriptor {
            FeatureDescriptor::builtin("test-initialization", "Test initialization")
        }
        fn validate_transport(&self, context: &InitializationContext<'_>) -> Result<(), String> {
            Self::decode(context.data).map(|_| ())
        }
        fn bind_manifest(
            &self,
            manifest: &mut WorkerManifest,
            context: &InitializationContext<'_>,
        ) -> Result<(), String> {
            self.bound.fetch_add(1, Ordering::SeqCst);
            manifest.engine.max_tokens = Some(Self::decode(context.data)?.tokens);
            Ok(())
        }
        fn validate_manifest(
            &self,
            manifest: &WorkerManifest,
            context: &InitializationContext<'_>,
        ) -> Result<(), String> {
            if manifest.engine.max_tokens != Some(Self::decode(context.data)?.tokens) {
                return Err("saved test Feature configuration mismatch".into());
            }
            Ok(())
        }
        fn prepare_host(
            &self,
            worker: &TestWorker,
            data: &FeatureInitializations,
        ) -> Result<(), WorkerError> {
            if worker.manifest().engine.max_tokens
                != Some(
                    Self::decode(data)
                        .map_err(WorkerError::FeatureLifecycle)?
                        .tokens,
                )
            {
                return Err(WorkerError::FeatureLifecycle(
                    "test Host configuration mismatch".into(),
                ));
            }
            Ok(())
        }
        fn apply(
            &self,
            worker: &TestWorker,
            _: &FeatureInitializations,
            phase: InitializationPhase,
        ) -> Result<(), WorkerError> {
            self.effects
                .lock()
                .unwrap()
                .push((worker.session_id(), phase));
            self.initialized.store(true, Ordering::SeqCst);
            Ok(())
        }
        fn requires_source_repair(
            &self,
            _: &WorkerMetadata,
            _: &FeatureInitializations,
        ) -> Result<bool, String> {
            Ok(false)
        }
    }

    #[derive(Clone)]
    struct GuardedModel {
        client: MockClient,
        initialized: Arc<AtomicBool>,
        called: mpsc::Sender<()>,
    }
    #[async_trait]
    impl LlmClient for GuardedModel {
        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }
        async fn stream(
            &self,
            request: Request,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmEvent, ClientError>> + Send>>, ClientError>
        {
            assert!(
                self.initialized.load(Ordering::SeqCst),
                "initial Submit must not precede Feature initialization"
            );
            self.called.send(()).unwrap();
            self.client.stream(request).await
        }
    }

    fn data(tokens: u32) -> FeatureInitializations {
        FeatureInitializations::new(vec![
            FeatureInitialization::encode(FEATURE, 1, &LaunchValue { tokens }).unwrap(),
        ])
        .unwrap()
    }

    #[test]
    #[serial_test::serial(worker_allocation)]
    fn alternate_provider_uses_runtime_create_metadata_reopen_and_saved_composition_restore() {
        require_restore_test_environment();
        let root = tempfile::tempdir().unwrap();
        let runtime_root = root.path().join("runtime");
        let probe = Probe::default();
        let (called, observed) = mpsc::channel();
        let model = GuardedModel {
            client: MockClient::new(simple_text_events()),
            initialized: probe.initialized.clone(),
            called,
        };
        let factory = |probe: Probe| {
            let model = model.clone();
            ProfileRuntimeWorkerFactory::new(root.path().join("no-current-profile"))
                .with_runtime_store_dir(&runtime_root)
                .with_runtime_id("initialization-runtime")
                .with_spawn_model_client_factory(move || Box::new(model.clone()))
                .with_feature_initialization_registry(Arc::new(
                    feature_initializers().with_adapter(probe),
                ))
        };
        let backend = Arc::new(WorkerRuntimeExecutionBackend::new(factory(probe.clone())).unwrap());
        let options = || {
            crate::FsRuntimeStoreOptions::new(&runtime_root)
                .with_runtime_id("initialization-runtime")
        };
        let runtime =
            EmbeddedRuntime::with_fs_store_and_execution_backend(options(), backend.clone())
                .unwrap();
        let mut request = create_request("alternate Feature");
        request.config_bundle = None;
        request.profile_source = ProfileSourceArchiveSource::Embedded { archive:
            crate::profile_archive::ProfileSourceArchive::build_evaluated_profile(
                "profile-source-archive:initialization".into(), "builtin:companion".into(),
                serde_json::json!({"slug":"initialization", "description":"test provider", "scope":"workspace_read",
                    "model":{"scheme":"anthropic", "model_id":"unused", "auth":{"kind":"none"}},
                    "engine":{"max_tokens":100}, "feature":{}})
            ).unwrap()
        };
        request.feature_initialization = data(173);
        request.initial_input = Some(WorkerInput::user("first input"));
        // Exercise the same JSON wire boundary as a remote Runtime caller.
        let wire: runtime_api::CreateWorkerRequest =
            serde_json::from_value(serde_json::to_value(&request).unwrap()).unwrap();
        let request: CreateWorkerRequest =
            serde_json::from_value(serde_json::to_value(wire).unwrap()).unwrap();
        let detail = runtime.create_worker(request.clone()).unwrap();
        observed
            .recv_timeout(Duration::from_secs(10))
            .expect("initial model dispatch after initialization");
        assert_eq!(probe.bound.load(Ordering::SeqCst), 1);
        runtime.stop_worker(&detail.worker_ref, None).unwrap();
        let name = ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(&detail.worker_ref);
        let aggregate_root = runtime_root
            .join("workers")
            .join(detail.worker_ref.worker_id.to_string());
        let metadata_store = WorkerAggregateStore::open_read_only(&aggregate_root, &name).unwrap();
        let metadata = metadata_store.read_by_name(&name).unwrap().unwrap();
        let saved = metadata.resolved_manifest_snapshot.clone();
        let file = runtime_root
            .join("workers")
            .join(detail.worker_ref.worker_id.to_string())
            .join("worker.json");
        let document: serde_json::Value =
            serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(
            document["execution_state"]["execution"]["request"]["feature_initialization"],
            serde_json::to_value(&request.feature_initialization).unwrap()
        );
        drop(runtime);
        drop(backend);

        let fresh_probe = Probe {
            initialized: probe.initialized.clone(),
            effects: probe.effects.clone(),
            ..Probe::default()
        };
        let backend =
            Arc::new(WorkerRuntimeExecutionBackend::new(factory(fresh_probe.clone())).unwrap());
        let runtime =
            EmbeddedRuntime::with_fs_store_and_execution_backend(options(), backend.clone())
                .unwrap();
        runtime
            .restore_worker(
                &detail.worker_ref,
                runtime.test_restore_request(&detail.worker_ref),
            )
            .unwrap();
        assert_eq!(
            fresh_probe.bound.load(Ordering::SeqCst),
            0,
            "restore must not bind current configuration again"
        );
        runtime.stop_worker(&detail.worker_ref, None).unwrap();
        let restored = metadata_store.read_by_name(&name).unwrap().unwrap();
        assert_eq!(restored.active, metadata.active);
        assert_eq!(restored.resolved_manifest_snapshot, saved);
        assert_eq!(
            probe.effects.lock().unwrap().as_slice(),
            &[
                (
                    metadata.active.as_ref().unwrap().session_id,
                    InitializationPhase::New
                ),
                (
                    metadata.active.as_ref().unwrap().session_id,
                    InitializationPhase::Restored
                ),
            ]
        );
        assert!(!root.path().join("no-current-profile").exists());
    }

    #[test]
    fn selected_initialization_plan_rejects_missing_duplicate_and_incompatible_providers() {
        let probe = Probe::default();
        let missing = feature_initializers().plan(&data(173)).unwrap_err();
        assert!(missing.contains("no selected initialization provider"));
        let duplicate = feature_initializers()
            .with_adapter(probe.clone())
            .with_adapter(probe.clone())
            .plan(&data(173))
            .unwrap_err();
        assert!(duplicate.contains("registered more than once"));
        let wrong_version = FeatureInitializations::new(vec![
            FeatureInitialization::encode(FEATURE, 2, &LaunchValue { tokens: 173 }).unwrap(),
        ])
        .unwrap();
        let registry = feature_initializers().with_adapter(probe);
        let context = InitializationContext {
            data: &wrong_version,
            workspace_id: None,
            memory_settings: None,
            backend_job: None,
        };
        assert!(registry.validate_transport(&context).is_err());
    }
}
