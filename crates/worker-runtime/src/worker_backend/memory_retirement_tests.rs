mod memory_retirement_tests {
    use super::*;

    // These fixtures start at the storage migration's output contract: the
    // resolved Manifest has no feature.memory. Actual old-store migration and
    // immutable receipt/archive preservation are exercised by fs_store tests.
    fn migrated_policy() -> (WorkerManifest, CreateWorkerRequest, WorkerMetadata) {
        let root = tempfile::tempdir().unwrap();
        let worker_ref = WorkerRef::new(WorkerId::now_v7());
        let (mut saved, request) = saved_subjektiv_policy(root.path(), &worker_ref);
        saved.feature.subjektiv.profile.extraction.threshold = Some(12_345);
        saved.feature.subjektiv.profile.extraction.worker_max_turns = Some(7);
        saved.feature.subjektiv.profile.chat.enabled = true;
        let mut metadata = WorkerMetadata::new(&saved.worker.name, None);
        metadata.resolved_manifest_snapshot =
            Some(manifest::write_persisted_worker_manifest_snapshot(&saved).unwrap());
        (saved, request, metadata)
    }

    #[test]
    fn migrated_subject_snapshot_retains_frozen_settings_and_policy() {
        let (saved, request, metadata) = migrated_policy();
        let before = serde_json::to_value(&request).unwrap();
        let snapshot = metadata.resolved_manifest_snapshot.clone().unwrap();
        assert!(snapshot["manifest"]["feature"].get("memory").is_none());
        validate_fixture_restore_metadata(&metadata, &request).unwrap();
        let (restored, _) =
            ProfileRuntimeWorkerFactory::manifest_for_restore(&metadata, &request).unwrap();
        assert_eq!(restored.feature.subjektiv, saved.feature.subjektiv);
        assert!(restored.feature.subjektiv.execution_enabled());
        assert_eq!(
            restored.feature.subjektiv.workspace_settings(),
            request.memory_settings
        );
        // All policy, including extraction opt-out/threshold and Subject
        // activation used by the resident surface, remains saved authority.
        assert!(!restored.feature.subjektiv.profile.extraction.enabled);
        assert_eq!(
            manifest::write_persisted_worker_manifest_snapshot(&restored).unwrap(),
            snapshot
        );
        assert_eq!(serde_json::to_value(&request).unwrap(), before);

        let mut wrong = request.clone();
        wrong.memory_settings.as_mut().unwrap().language = "changed since create".into();
        assert!(
            ProfileRuntimeWorkerFactory::manifest_for_restore(&metadata, &wrong)
                .err()
                .unwrap()
                .contains("subjektiv settings snapshot mismatch")
        );
        wrong = request.clone();
        wrong.memory_settings = None;
        assert!(
            ProfileRuntimeWorkerFactory::manifest_for_restore(&metadata, &wrong)
                .err()
                .unwrap()
                .contains("missing its trusted settings snapshot")
        );
        wrong = request;
        wrong.feature_initialization = Default::default();
        assert!(
            ProfileRuntimeWorkerFactory::manifest_for_restore(&metadata, &wrong)
                .err()
                .unwrap()
                .contains("unauthorized subjektiv attachment")
        );
    }

    #[test]
    fn historical_ordinary_settings_are_receipt_only_on_restore_not_create() {
        let (mut saved, mut request, mut metadata) = migrated_policy();
        saved.feature.subjektiv.workspace_settings = None;
        request.feature_initialization = Default::default();
        metadata.resolved_manifest_snapshot =
            Some(manifest::write_persisted_worker_manifest_snapshot(&saved).unwrap());
        let before = serde_json::to_value(&request).unwrap();
        assert!(request.memory_settings.is_some());
        assert!(
            feature_initialization_context(&request)
                .memory_settings
                .is_some()
        );
        assert!(
            restore_feature_initialization_context(&request)
                .memory_settings
                .is_none()
        );
        assert!(
            bind_worker_initialization(&mut saved.clone(), &request)
                .unwrap_err()
                .contains("require an explicit Subjektiv connection"),
            "new creates must not gain the historical restore allowance"
        );
        let (restored, _) =
            ProfileRuntimeWorkerFactory::manifest_for_restore(&metadata, &request).unwrap();
        assert!(!restored.feature.subjektiv.execution_enabled());
        assert_eq!(restored.feature.subjektiv.workspace_settings(), None);
        assert_eq!(
            restored.feature.subjektiv.profile,
            saved.feature.subjektiv.profile
        );
        assert_eq!(serde_json::to_value(&request).unwrap(), before);
    }

    #[test]
    fn restore_context_preserves_explicit_invalid_subjektiv_initialization() {
        let (_, mut request, metadata) = migrated_policy();
        let entry = protocol::feature_initialization::FeatureInitialization::encode(
            subjektiv_api::worker_initialization::FEATURE_ID,
            subjektiv_api::worker_initialization::VERSION + 1,
            &serde_json::json!({}),
        )
        .unwrap();
        request.feature_initialization =
            protocol::feature_initialization::FeatureInitializations::new(vec![entry]).unwrap();
        assert_eq!(
            restore_feature_initialization_context(&request).memory_settings,
            request.memory_settings.as_ref()
        );
        let error = ProfileRuntimeWorkerFactory::manifest_for_restore(&metadata, &request)
            .err()
            .expect("invalid explicit initialization must not become an ordinary restore");
        assert!(
            error.contains("unsupported Feature initialization epoch"),
            "{error}"
        );
    }

    #[test]
    fn remaining_retired_snapshots_are_refused_without_decoder_repair() {
        let (_, request, metadata) = migrated_policy();
        for legacy in [
            serde_json::json!({"profile":{"enabled":true}}),
            serde_json::json!({"profile":{"enabled":false}}),
            serde_json::json!({"profile":{}}),
            serde_json::Value::Null,
        ] {
            for wrapped in [true, false] {
                let mut snapshot = metadata.resolved_manifest_snapshot.clone().unwrap();
                snapshot["manifest"]["feature"]["memory"] = legacy.clone();
                if !wrapped {
                    snapshot = snapshot["manifest"].clone();
                }
                assert!(
                    manifest::read_persisted_worker_manifest_snapshot(snapshot.clone()).is_err()
                );
                let mut metadata = metadata.clone();
                metadata.resolved_manifest_snapshot = Some(snapshot.clone());
                let error = ProfileRuntimeWorkerFactory::manifest_for_restore(&metadata, &request)
                    .err()
                    .expect("retired snapshots cannot execute, even when disabled");
                assert!(error.contains("retired feature.memory"), "{error}");
                if legacy.pointer("/profile/enabled") == Some(&serde_json::json!(false)) {
                    assert!(error.contains("Runtime storage migration"), "{error}");
                } else {
                    assert!(error.contains("replacement Worker"), "{error}");
                }
                assert_eq!(metadata.resolved_manifest_snapshot, Some(snapshot));
            }
        }
    }

    #[test]
    fn retired_consolidation_profile_cannot_restore_even_without_old_memory_fields() {
        let (_, mut request, metadata) = migrated_policy();
        for profile in [
            ProfileSelector::Builtin("memory-consolidation".into()),
            ProfileSelector::Builtin("builtin:memory-consolidation".into()),
            ProfileSelector::Named("memory-consolidation".into()),
            ProfileSelector::Named("builtin:memory-consolidation".into()),
        ] {
            request.profile = profile;
            let before = serde_json::to_value(&request).unwrap();
            let error = ProfileRuntimeWorkerFactory::manifest_for_restore(&metadata, &request)
                .err()
                .expect("retired Profile must not execute a migrated snapshot or fallback");
            assert!(
                error.contains("retired Workspace memory-consolidation Profile"),
                "{error}"
            );
            assert_eq!(serde_json::to_value(&request).unwrap(), before);
        }
        request.profile = ProfileSelector::Builtin("subjektiv-memory-consolidation".into());
        ProfileRuntimeWorkerFactory::reject_retired_memory_profile(&request).unwrap();
    }

    #[test]
    fn saved_retired_profile_provenance_refuses_supported_request_selector() {
        let (_, request, metadata) = migrated_policy();
        ProfileRuntimeWorkerFactory::reject_retired_memory_profile(&request).unwrap();
        let before_request = serde_json::to_value(&request).unwrap();
        for name in ["memory-consolidation", "builtin:memory-consolidation"] {
            for wrapped in [true, false] {
                let mut snapshot = metadata.resolved_manifest_snapshot.clone().unwrap();
                snapshot["manifest"]["profile"] = serde_json::json!({
                    "source": {"kind":"registry", "source":"builtin", "name":name}
                });
                if !wrapped {
                    snapshot = snapshot["manifest"].clone();
                }
                // Provenance alone is valid Manifest data, not a retired field
                // for the strict decoder to reject. Restore must check it too.
                manifest::read_persisted_worker_manifest_snapshot(snapshot.clone()).unwrap();
                let mut metadata = metadata.clone();
                metadata.resolved_manifest_snapshot = Some(snapshot.clone());
                let error = ProfileRuntimeWorkerFactory::manifest_for_restore(&metadata, &request)
                    .err()
                    .expect("saved retired Profile must not execute via a supported selector");
                assert!(
                    error.contains("memory-consolidation Profile provenance"),
                    "{error}"
                );
                assert_eq!(metadata.resolved_manifest_snapshot, Some(snapshot));
                assert_eq!(serde_json::to_value(&request).unwrap(), before_request);
            }
        }
        let mut metadata = metadata;
        metadata.resolved_manifest_snapshot.as_mut().unwrap()["manifest"]["profile"] = serde_json::json!({
            "source": {"kind":"registry", "source":"builtin", "name":"subjektiv-memory-consolidation"}
        });
        ProfileRuntimeWorkerFactory::manifest_for_restore(&metadata, &request).unwrap();
    }

    // Connect the production preflight AND later candidate validation to a real
    // persisted Session. Only model I/O is substituted; no current Profile is
    // resolved and no Subject or legacy Memory Host exists for this ordinary
    // historical Workspace receipt.
    #[cfg(feature = "http-server")]
    async fn ordinary_restore_fixture(root: &Path) -> WorkerExecutionRestoreRequest {
        require_restore_test_environment();
        let worker_ref = WorkerRef::new(WorkerId::now_v7());
        let name = ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(&worker_ref);
        let aggregate = root.join("workers").join(worker_ref.worker_id.to_string());
        let session = WorkerSessionStore::new(aggregate.join("session")).unwrap();
        let store = WorkerAggregateStore::new(&aggregate, &name).unwrap();
        let (mut saved, mut request) = saved_subjektiv_policy(root, &worker_ref);
        saved.feature.subjektiv.workspace_settings = None;
        request.feature_initialization = Default::default();
        let mut engine =
            Engine::<_, agen::state::Mutable, worker::SessionHistoryMetadata>::new_annotated(
                MockClient::new(Vec::new()),
            );
        engine.set_system_prompt("Frozen ordinary Worker Session prompt");
        let mut worker = Worker::new(
            saved,
            engine,
            CombinedStore::new(session, store),
            WorkerWorkspaceContext::unavailable(
                Some(WorkspaceId::new("workspace-saved-subjektiv").unwrap()),
                "ordinary historical receipt must not inject Memory",
            ),
            WorkerFilesystemAuthority::None,
            Scope::empty(),
        )
        .await
        .unwrap();
        worker.enable_worker_metadata_write_through().unwrap();
        worker.materialize_durable_session_head().await.unwrap();
        drop(worker);
        let mut restore = direct_restore_request(worker_ref);
        restore.request = request;
        restore.workspace_scope = Some(crate::runtime::RuntimeWorkspaceScope::new(
            "workspace-saved-subjektiv",
            "server-main",
        ));
        restore
    }

    #[cfg(feature = "http-server")]
    #[tokio::test]
    #[serial_test::serial(worker_allocation)]
    async fn historical_ordinary_restore_reaches_controller_without_memory_injection() {
        let root = tempfile::tempdir().unwrap();
        let runtime_store = root.path().join("runtime");
        let request = ordinary_restore_fixture(&runtime_store).await;
        let before_request = serde_json::to_value(&request.request).unwrap();
        let name = ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(&request.worker_ref);
        let store = WorkerAggregateStore::open_read_only(
            runtime_store
                .join("workers")
                .join(request.worker_ref.worker_id.to_string()),
            &name,
        )
        .unwrap();
        let before = store.read_by_name(&name).unwrap().unwrap();
        let model = MockClient::new(Vec::new());
        let injected = model.clone();
        let factory = ProfileRuntimeWorkerFactory::new(root.path().join("no-current-profile"))
            .with_runtime_store_dir(&runtime_store)
            .with_runtime_id("memory-retirement-test")
            .with_spawn_model_client_factory(move || Box::new(injected.clone()));
        factory.preflight_restore(&request).await.unwrap();
        let controller = factory
            .restore_controller(request.clone())
            .await
            .unwrap_or_else(|error| panic!("historical ordinary restore failed: {error}"));
        assert_eq!(
            controller.handle.shared_state.catalog_status(),
            WorkerStatus::Idle
        );
        shutdown_profile_controller(controller).await;
        assert_eq!(model.call_count.load(Ordering::SeqCst), 0);
        assert_eq!(
            serde_json::to_value(&request.request).unwrap(),
            before_request
        );
        let after = store.read_by_name(&name).unwrap().unwrap();
        assert_eq!(
            after.resolved_manifest_snapshot,
            before.resolved_manifest_snapshot
        );
        assert_eq!(after.active, before.active);
        assert!(after.subjektiv_session_attribution.is_none());
        assert!(!root.path().join("no-current-profile").exists());
    }

    #[cfg(feature = "http-server")]
    #[tokio::test]
    #[serial_test::serial(worker_allocation)]
    async fn enabled_legacy_snapshot_and_profile_refuse_before_execution_resources() {
        let root = tempfile::tempdir().unwrap();
        let runtime_store = root.path().join("runtime");
        let mut request = ordinary_restore_fixture(&runtime_store).await;
        let name = ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(&request.worker_ref);
        let aggregate = runtime_store
            .join("workers")
            .join(request.worker_ref.worker_id.to_string());
        let store = WorkerAggregateStore::new(&aggregate, &name).unwrap();
        let original = store.read_by_name(&name).unwrap().unwrap();
        let model = MockClient::new(Vec::new());
        let injected = model.clone();
        let factory = ProfileRuntimeWorkerFactory::new(root.path())
            .with_runtime_store_dir(&runtime_store)
            .with_runtime_id("memory-retirement-test")
            .with_spawn_model_client_factory(move || Box::new(injected.clone()));
        for profile in [false, true] {
            let mut metadata = original.clone();
            if profile {
                request.request.profile = ProfileSelector::Builtin("memory-consolidation".into());
            } else {
                metadata.resolved_manifest_snapshot.as_mut().unwrap()["manifest"]["feature"]["memory"] =
                    serde_json::json!({"profile":{"enabled":true}});
            }
            store.write(&metadata).unwrap();
            let before = persisted_files(&runtime_store);
            let failure = factory.preflight_restore(&request).await.unwrap_err();
            let diagnostic = failure.legacy_diagnostic.as_deref().unwrap();
            assert!(
                diagnostic.contains(if profile {
                    "memory-consolidation Profile"
                } else {
                    "retired feature.memory"
                }),
                "{diagnostic}"
            );
            let failure = match factory.restore_controller(request.clone()).await {
                Ok(controller) => {
                    shutdown_profile_controller(controller).await;
                    panic!("retired execution reached Controller");
                }
                Err(failure) => failure,
            };
            assert_eq!(failure.stage, RuntimeRestoreStage::Preflight);
            assert_eq!(persisted_files(&runtime_store), before);
            assert!(!aggregate.join("runs").exists());
            assert!(factory.observation_hub.workers.lock().unwrap().is_empty());
            assert_eq!(model.call_count.load(Ordering::SeqCst), 0);
        }
    }
}
