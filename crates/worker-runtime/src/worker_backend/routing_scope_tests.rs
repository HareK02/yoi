mod routing_scope_tests {
    //! Production local router/provider composition; Host domain authorization is
    //! outside this component suite. Restore uses the standalone client explicitly.
    use super::*;

    #[cfg(target_os = "linux")]
    mod factory_routing {
        //! Real Profile factory, saved Manifest/Session, Controller tool-session
        //! view and local provider. Only the model boundary and an initialization
        //! adapter are substitutes; the adapter captures live scope and adds an
        //! explicit deny. Launch policy supplies the .worktree deny. Linux with
        //! the isolated restore/allocation environment is required; no Workspace
        //! Host authorization or model-driven tool dispatch is claimed.
        use super::*;
        use protocol::feature_initialization::{FeatureInitialization, FeatureInitializations};
        use worker::feature::FeatureDescriptor;
        use worker::feature::initialization::{
            FeatureInitializationAdapter, InitializationContext, InitializationPhase,
        };

        const FEATURE: &str = "builtin:test-routing-scope";
        type TestWorker = Worker<Box<dyn LlmClient>, ProfileRestoreStore>;

        #[derive(serde::Serialize, serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct LaunchValue {}

        struct Installed {
            session_id: session_store::SessionId,
            phase: InitializationPhase,
            scope: manifest::SharedScope,
        }

        #[derive(Clone)]
        struct Probe {
            root: PathBuf,
            installed: Arc<Mutex<Vec<Installed>>>,
        }

        impl FeatureInitializationAdapter<Box<dyn LlmClient>, ProfileRestoreStore> for Probe {
            fn descriptor(&self) -> FeatureDescriptor {
                FeatureDescriptor::builtin("test-routing-scope", "Routing scope probe")
            }

            fn validate_transport(
                &self,
                context: &InitializationContext<'_>,
            ) -> Result<(), String> {
                context
                    .data
                    .get(FEATURE)
                    .ok_or("routing probe missing")?
                    .decode::<LaunchValue>(FEATURE, 1)
                    .map(|_| ())
            }

            fn bind_manifest(
                &self,
                manifest: &mut WorkerManifest,
                _: &InitializationContext<'_>,
            ) -> Result<(), String> {
                manifest.scope.deny.push(manifest::ScopeRule {
                    target: self.root.join("nested/denied"),
                    permission: manifest::Permission::Read,
                    recursive: true,
                    symlink_policy: manifest::SymlinkPolicy::Resolved,
                });
                Ok(())
            }

            fn validate_manifest(
                &self,
                _: &WorkerManifest,
                context: &InitializationContext<'_>,
            ) -> Result<(), String> {
                self.validate_transport(context)
            }

            fn prepare_host(
                &self,
                _: &TestWorker,
                _: &FeatureInitializations,
            ) -> Result<(), WorkerError> {
                Ok(())
            }

            fn apply(
                &self,
                worker: &TestWorker,
                _: &FeatureInitializations,
                phase: InitializationPhase,
            ) -> Result<(), WorkerError> {
                self.installed.lock().unwrap().push(Installed {
                    session_id: worker.session_id(),
                    phase,
                    scope: worker.scope().clone(),
                });
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

        async fn assert_installed_policy(
            installed: &Installed,
            controller: &RuntimeWorkerController,
            workdir_id: &str,
            root: &Path,
            cwd: Option<&str>,
            label: &str,
        ) {
            // Fresh initialization runs before bind_workdir_sessions. Observe
            // the actual Controller view, not that constructor fallback router.
            let session = controller
                .handle
                .shared_state
                .fs_view()
                .expect("Controller filesystem view")
                .session()
                .clone();
            assert_eq!(session.workdir().id().as_str(), workdir_id);
            assert_eq!(session.environment().cwd.as_str(), cwd.unwrap_or(""));
            let directory = root.join(cwd.unwrap_or(""));
            let expected_marker: &[u8] = if cwd.is_some() { b"nested" } else { b"root" };
            let denied_prefix = if cwd.is_some() {
                "denied"
            } else {
                "nested/denied"
            };
            let read = |path: &str| workdir::ReadRequest {
                path: workdir::WorkdirPath::new(path).unwrap(),
                offset: 0,
                limit: 10,
                max_bytes: 100,
            };
            let marker = session.read(read("marker.txt")).await.unwrap();
            assert_eq!(
                marker.bytes, expected_marker,
                "{label}: tools must use attachment cwd"
            );
            assert_eq!(marker.path.as_str(), "marker.txt");
            let allowed = format!("allowed-{label}");
            session.write(write(&allowed)).await.unwrap();
            assert_eq!(fs::read(directory.join(&allowed)).unwrap(), b"changed");
            if cwd.is_some() {
                assert!(!root.join(&allowed).exists());
            }
            let denied = session
                .read(read(&format!("{denied_prefix}/secret")))
                .await
                .unwrap_err();
            assert!(denied.denial_reason().is_some(), "{label}: {denied:?}");
            let denied = session
                .write(write(&format!("{denied_prefix}/new")))
                .await
                .unwrap_err();
            assert!(denied.denial_reason().is_some(), "{label}: {denied:?}");
            assert!(!root.join("nested/denied/new").exists());
            assert_eq!(
                fs::read(root.join("nested/denied/secret")).unwrap(),
                b"secret"
            );

            if cwd.is_none() {
                let denied = session.write(write(".worktree/escape")).await.unwrap_err();
                assert!(denied.denial_reason().is_some(), "{label}: {denied:?}");
                assert!(!root.join(".worktree/escape").exists());
            }

            // Mutate the exact live SharedScope captured from the Worker, after
            // opening and successfully writing through its installed session.
            installed
                .scope
                .update(|_| {
                    manifest::Scope::from_config(&manifest::ScopeConfig {
                        allow: vec![manifest::ScopeRule {
                            target: root.to_path_buf(),
                            permission: manifest::Permission::Read,
                            recursive: true,
                            symlink_policy: manifest::SymlinkPolicy::Resolved,
                        }],
                        deny: vec![manifest::ScopeRule {
                            target: root.join("nested/denied"),
                            permission: manifest::Permission::Read,
                            recursive: true,
                            symlink_policy: manifest::SymlinkPolicy::Resolved,
                        }],
                    })
                })
                .unwrap();
            let denied = session.write(write("after-narrowing")).await.unwrap_err();
            assert!(denied.denial_reason().is_some(), "{label}: {denied:?}");
            assert!(!directory.join("after-narrowing").exists());
            assert_eq!(
                session.read(read("marker.txt")).await.unwrap().bytes,
                expected_marker
            );
        }

        #[tokio::test]
        #[serial_test::serial(worker_allocation)]
        async fn factory_fresh_and_same_id_restore_install_cwd_denies_and_live_scope() {
            require_restore_test_environment();
            for cwd in [None, Some("nested")] {
                let runtime_base = tempfile::tempdir().unwrap();
                let repo = create_clean_repo();
                let materializer = RuntimeGitMaterializer::new(runtime_base.path());
                let binding = materializer
                    .create(&working_directory_request(repo.path()))
                    .unwrap();
                let root = binding.root().to_path_buf();
                fs::create_dir_all(root.join("nested/denied")).unwrap();
                fs::create_dir_all(root.join(".worktree")).unwrap();
                fs::write(root.join("marker.txt"), "root").unwrap();
                fs::write(root.join("nested/marker.txt"), "nested").unwrap();
                fs::write(root.join("nested/denied/secret"), "secret").unwrap();
                let id = binding.working_directory.id.clone();
                drop(binding);
                let alias = WorkdirAttachmentAlias::new("checkout").unwrap();
                let installed = Arc::new(Mutex::new(Vec::new()));
                let store = runtime_base.path().join("worker-store");
                let factory = || {
                    ProfileRuntimeWorkerFactory::new(runtime_base.path().join("no-profile"))
                        .with_runtime_store_dir(&store)
                        .with_spawn_model_client_factory(|| Box::new(MockClient::new(vec![])))
                        .with_feature_initialization_registry(Arc::new(
                            feature_initializers().with_adapter(Probe {
                                root: root.clone(),
                                installed: installed.clone(),
                            }),
                        ))
                };
                let mut request = create_request("factory routing scope");
                request.config_bundle = None;
                request.profile = ProfileSelector::Builtin("companion".into());
                request.profile_source = ProfileSourceArchiveSource::Embedded {
                    archive: crate::profile_archive::ProfileSourceArchive::build_evaluated_profile(
                        "profile-source-archive:routing-scope".into(),
                        "builtin:companion".into(),
                        serde_json::json!({"slug":"routing-scope", "description":"routing fixture",
                            "scope":"workspace_read", "model":{"scheme":"anthropic",
                            "model_id":"unused", "auth":{"kind":"none"}},
                            "engine":{"max_tokens":100}, "feature":{}}),
                    )
                    .unwrap(),
                };
                request.feature_initialization = FeatureInitializations::new(vec![
                    FeatureInitialization::encode(FEATURE, 1, &LaunchValue {}).unwrap(),
                ])
                .unwrap();
                request.workdir_attachments = vec![WorkingDirectoryAttachmentClaim {
                    provider: crate::catalog::WorkdirAttachmentProvider::Runtime,
                    alias: alias.clone(),
                    working_directory_id: id.clone(),
                    relative_cwd: cwd.map(str::to_string),
                    capabilities: WorkdirSessionCapabilities::ALL,
                }];
                let worker_ref = WorkerRef::new(request.worker_id);
                let fresh_factory = factory();
                let controller = fresh_factory
                    .spawn_controller(WorkerExecutionSpawnRequest {
                        worker_ref: worker_ref.clone(),
                        request: request.clone(),
                        workspace_scope: None,
                        context: test_execution_context(worker_ref.clone()),
                        workdir_attachments: BTreeMap::from([(
                            alias.clone(),
                            materializer.bind_working_directory(&id, cwd).unwrap(),
                        )]),
                        config_bundle: None,
                    })
                    .await
                    .unwrap_or_else(|error| panic!("fresh factory failed: {error}"));
                let fresh = installed
                    .lock()
                    .unwrap()
                    .pop()
                    .expect("fresh initialization");
                assert_eq!(fresh.phase, InitializationPhase::New);
                assert_installed_policy(&fresh, &controller, &id, &root, cwd, "fresh").await;
                shutdown_profile_controller(controller).await;
                drop(fresh_factory);
                let name = ProfileRuntimeWorkerFactory::runtime_worker_name_for_ref(&worker_ref);
                let metadata_store = WorkerAggregateStore::open_read_only(
                    store.join("workers").join(worker_ref.worker_id.to_string()),
                    &name,
                )
                .unwrap();
                let saved = metadata_store.read_by_name(&name).unwrap().unwrap();
                assert_eq!(saved.active.as_ref().unwrap().session_id, fresh.session_id);

                // Reconstruct from the saved Manifest/Session with a new factory;
                // do not seed another Worker or recreate a router in the test.
                let restored_factory = factory();
                let mut restore = direct_restore_request(worker_ref.clone());
                restore.request = request;
                restore.workdir_attachments = BTreeMap::from([(
                    alias.clone(),
                    materializer.bind_working_directory(&id, cwd).unwrap(),
                )]);
                let controller = restored_factory
                    .restore_controller(restore)
                    .await
                    .unwrap_or_else(|error| {
                        panic!("saved same-ID factory restore failed: {error}")
                    });
                let restored = installed
                    .lock()
                    .unwrap()
                    .pop()
                    .expect("restore initialization");
                assert_eq!(restored.phase, InitializationPhase::Restored);
                assert_eq!(restored.session_id, fresh.session_id);
                assert_installed_policy(&restored, &controller, &id, &root, cwd, "restored").await;
                shutdown_profile_controller(controller).await;
                let after = metadata_store.read_by_name(&name).unwrap().unwrap();
                assert_eq!(after.active, saved.active);
                assert_eq!(
                    after.resolved_manifest_snapshot,
                    saved.resolved_manifest_snapshot
                );
                assert!(installed.lock().unwrap().is_empty());
            }
        }
    }

    fn write(path: &str) -> workdir::WriteRequest {
        workdir::WriteRequest {
            path: workdir::WorkdirPath::new(path).unwrap(),
            content: b"changed".to_vec(),
            expected_hash: None,
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn local_and_restored_routers_retain_denies_and_live_worker_narrowing() {
        let runtime_base = tempfile::tempdir().unwrap();
        let repo = create_clean_repo();
        let materializer = RuntimeGitMaterializer::new(runtime_base.path());
        let binding = materializer
            .create(&working_directory_request(repo.path()))
            .unwrap();
        let root = binding.root().to_path_buf();
        let rule = |target, permission| manifest::ScopeRule {
            target,
            permission,
            recursive: true,
            symlink_policy: manifest::SymlinkPolicy::Resolved,
        };
        let scope = manifest::SharedScope::new(
            manifest::Scope::from_config(&manifest::ScopeConfig {
                allow: vec![rule(root.clone(), manifest::Permission::Write)],
                deny: vec![
                    rule(root.join("denied"), manifest::Permission::Write),
                    rule(root.join(".worktree"), manifest::Permission::Write),
                ],
            })
            .unwrap(),
        );
        let alias = WorkdirAttachmentAlias::new("checkout").unwrap();
        let attachments = BTreeMap::from([(alias.clone(), binding)]);
        let caps = BTreeMap::from([(alias.clone(), WorkdirSessionCapabilities::ALL)]);
        let first =
            runtime_local_workdir_router(&attachments, &caps, None, Some(scope.clone())).unwrap();
        let claims = [WorkingDirectoryAttachmentClaim {
            provider: crate::catalog::WorkdirAttachmentProvider::Runtime,
            alias: alias.clone(),
            working_directory_id: attachments[&alias].working_directory.id.clone(),
            relative_cwd: None,
            capabilities: WorkdirSessionCapabilities::ALL,
        }];
        let restored = restored_workdir_router(
            &attachments,
            &[],
            &BTreeMap::new(),
            &claims,
            WorkerWorkspaceContext::no_workspace().client_handle(),
            Some(scope.clone()),
        )
        .unwrap();
        for (index, router) in [&first, &restored].into_iter().enumerate() {
            let session = router.session(&alias).unwrap();
            session
                .write(write(&format!("allowed-{index}")))
                .await
                .unwrap();
            for denied in ["denied/file", ".worktree/file"] {
                let error = session.write(write(denied)).await.unwrap_err();
                assert!(error.denial_reason().is_some(), "{error:?}");
                assert!(!root.join(denied).exists());
            }
        }
        scope
            .update(|_| {
                manifest::Scope::from_config(&manifest::ScopeConfig {
                    allow: vec![rule(root.clone(), manifest::Permission::Read)],
                    deny: vec![],
                })
            })
            .unwrap();
        for router in [&first, &restored] {
            assert!(
                router
                    .session(&alias)
                    .unwrap()
                    .write(write("after-narrowing"))
                    .await
                    .unwrap_err()
                    .denial_reason()
                    .is_some()
            );
            router.close_all().await.unwrap();
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn deferred_restore_rebases_tools_and_keeps_delegation_root_relative() {
        let runtime_base = tempfile::tempdir().unwrap();
        let repo = create_clean_repo();
        let materializer = RuntimeGitMaterializer::new(runtime_base.path());
        let binding = materializer
            .create(&working_directory_request(repo.path()))
            .unwrap();
        fs::create_dir(binding.root().join("nested")).unwrap();
        fs::write(binding.root().join("marker.txt"), "root").unwrap();
        fs::write(binding.root().join("nested/marker.txt"), "nested").unwrap();
        let alias = WorkdirAttachmentAlias::new("checkout").unwrap();
        let id = binding.working_directory.id.clone();
        let mut request = direct_restore_request(WorkerRef::new(WorkerId::now_v7()));
        request.request.workdir_attachments = vec![WorkingDirectoryAttachmentClaim {
            provider: crate::catalog::WorkdirAttachmentProvider::Runtime,
            alias: alias.clone(),
            working_directory_id: id,
            relative_cwd: Some("nested".into()),
            capabilities: WorkdirSessionCapabilities::ALL,
        }];
        request.deferred_workdir_scope.set(None).unwrap();
        let deferred =
            deferred_restore_workdir_sessions(&request, Some(Arc::new(materializer))).unwrap();
        let router = restored_workdir_router(
            &BTreeMap::new(),
            &[],
            &deferred,
            &request.request.workdir_attachments,
            WorkerWorkspaceContext::no_workspace().client_handle(),
            None,
        )
        .unwrap();
        let session = router.session(&alias).unwrap();
        assert_eq!(session.environment().cwd.as_str(), "nested");
        let read = session
            .read(workdir::ReadRequest {
                path: workdir::WorkdirPath::new("marker.txt").unwrap(),
                offset: 0,
                limit: 10,
                max_bytes: 100,
            })
            .await
            .unwrap();
        assert_eq!(read.bytes, b"nested");
        assert_eq!(read.path.as_str(), "marker.txt");
        let glob = session
            .glob(workdir::GlobRequest {
                pattern: "*.txt".into(),
                path: workdir::WorkdirPath::root(),
                limit: 10,
            })
            .await
            .unwrap();
        assert_eq!(
            glob.paths,
            vec![workdir::WorkdirPath::new("marker.txt").unwrap()]
        );
        let grep = session
            .grep(workdir::GrepRequest {
                pattern: "nested".into(),
                path: workdir::WorkdirPath::root(),
                glob: None,
                file_type: None,
                case_insensitive: false,
                before_context: 0,
                after_context: 0,
                multiline: false,
                output_mode: workdir::GrepOutputMode::FilesWithMatches,
                limit: 10,
                offset: 0,
            })
            .await
            .unwrap();
        assert_eq!(
            grep.paths,
            vec![workdir::WorkdirPath::new("marker.txt").unwrap()]
        );
        let brokers = workdir::WorkdirToolBrokerRouter::new(router.clone());
        let child = brokers
            .scope(vec![(
                alias.clone(),
                workdir::WorkdirToolScope {
                    rules: vec![workdir::WorkdirToolScopeRule {
                        target: workdir::WorkdirPath::new("nested").unwrap(),
                        permission: workdir::WorkdirToolScopePermission::Read,
                        recursive: true,
                        symlink_policy: manifest::SymlinkPolicy::Resolved,
                    }],
                    cwd: workdir::WorkdirPath::new("nested").unwrap(),
                    command: false,
                },
            )])
            .await
            .unwrap();
        let child_read = child
            .session_router()
            .resolve(None)
            .unwrap()
            .session
            .read(workdir::ReadRequest {
                path: workdir::WorkdirPath::new("marker.txt").unwrap(),
                offset: 0,
                limit: 10,
                max_bytes: 100,
            })
            .await
            .unwrap();
        assert_eq!(child_read.bytes, b"nested");
        let command = session
            .start_command(workdir::CommandRequest {
                command: "cat marker.txt".into(),
                timeout_secs: 5,
                transport_byte_budget: 1024,
                cwd: workdir::WorkdirPath::root(),
                tool_call_id: None,
            })
            .await
            .unwrap();
        let output = tokio::time::timeout(Duration::from_secs(10), async {
            let mut cursor = 0;
            let mut bytes = Vec::new();
            loop {
                let output = session
                    .command_output(workdir::CommandOutputRequest {
                        handle: command.clone(),
                        cursor,
                        limit: 1024,
                        wait: true,
                    })
                    .await
                    .unwrap();
                cursor = output.next_cursor;
                for chunk in output.chunks {
                    bytes.extend(chunk.bytes);
                }
                if let Some(terminal) = output.terminal {
                    assert_eq!(terminal.exit_code, Some(0));
                    break bytes;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(output, b"nested");
        router.close_all().await.unwrap();
    }
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn deferred_open_requires_binding_and_retains_live_scope_after_open() {
        let runtime_base = tempfile::tempdir().unwrap();
        let repo = create_clean_repo();
        let materializer = RuntimeGitMaterializer::new(runtime_base.path());
        let binding = materializer
            .create(&working_directory_request(repo.path()))
            .unwrap();
        let root = binding.root().to_path_buf();
        fs::create_dir(root.join("denied")).unwrap();
        fs::write(root.join("denied/secret"), "secret").unwrap();
        fs::write(root.join("allowed"), "allowed").unwrap();
        let alias = WorkdirAttachmentAlias::new("checkout").unwrap();
        let mut request = direct_restore_request(WorkerRef::new(WorkerId::now_v7()));
        request.request.workdir_attachments = vec![WorkingDirectoryAttachmentClaim {
            provider: crate::catalog::WorkdirAttachmentProvider::Runtime,
            alias: alias.clone(),
            working_directory_id: binding.working_directory.id.clone(),
            relative_cwd: None,
            capabilities: WorkdirSessionCapabilities::ALL,
        }];
        let sessions =
            deferred_restore_workdir_sessions(&request, Some(Arc::new(materializer))).unwrap();
        let session = &sessions[&alias];
        let read = |path: &str| workdir::ReadRequest {
            path: workdir::WorkdirPath::new(path).unwrap(),
            offset: 0,
            limit: 10,
            max_bytes: 100,
        };
        assert_eq!(
            session
                .read(read("allowed"))
                .await
                .unwrap_err()
                .failure_reason(),
            Some(workdir::WorkdirFailureReason::SessionOpenFailed)
        );
        let rule = |target, permission| manifest::ScopeRule {
            target,
            permission,
            recursive: true,
            symlink_policy: manifest::SymlinkPolicy::Resolved,
        };
        let scope = manifest::SharedScope::new(
            manifest::Scope::from_config(&manifest::ScopeConfig {
                allow: vec![rule(root.clone(), manifest::Permission::Write)],
                deny: vec![
                    rule(root.join("denied"), manifest::Permission::Read),
                    rule(root.join(".worktree"), manifest::Permission::Write),
                ],
            })
            .unwrap(),
        );
        request
            .deferred_workdir_scope
            .set(Some(scope.clone()))
            .unwrap();
        let observed = session.read(read("allowed")).await.unwrap();
        assert_eq!(observed.bytes, b"allowed");
        assert!(
            session
                .read(read("denied/secret"))
                .await
                .unwrap_err()
                .denial_reason()
                .is_some()
        );
        assert!(
            session
                .write(write(".worktree/escape"))
                .await
                .unwrap_err()
                .denial_reason()
                .is_some()
        );
        let mut update = write("allowed");
        update.expected_hash = Some(observed.content_hash);
        session.write(update).await.unwrap();
        scope
            .update(|_| {
                manifest::Scope::from_config(&manifest::ScopeConfig {
                    allow: vec![rule(root, manifest::Permission::Read)],
                    deny: vec![],
                })
            })
            .unwrap();
        assert!(
            session
                .write(write("allowed"))
                .await
                .unwrap_err()
                .denial_reason()
                .is_some()
        );
        session.close().await.unwrap();
    }
}
