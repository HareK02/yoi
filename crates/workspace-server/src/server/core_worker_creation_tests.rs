mod core_worker_creation_tests {
    //! Authenticated creation, persisted grants and current operation checks use
    //! isolated real stores. Only Runtime execution is the shared workdirless fake.

    use super::*;
    use server_api::authorization::{OperationAuthorizationRequest, PermissionResource};
    use server_api::launch_policy::WorkerCreationAuthorization;

    async fn create_assignment(
        api: &WorkspaceApi,
        operation: &str,
        assignment: Option<feature_permissions::PermissionAssignment>,
    ) -> BrowserCreateWorkerResponse {
        let context = server_request_context(
            Some(test_owner_actor()),
            None,
            None,
            None,
            &HeaderMap::new(),
        );
        create_workspace_worker_authenticated(
            api.clone(),
            HeaderMap::new(),
            CreateWorkspaceWorkerRequest {
                runtime_id: WorkdirlessFixtureRuntime::RUNTIME_ID.into(),
                display_name: operation.into(),
                singleton_key: None,
                profile: Some("builtin:companion".into()),
                ticket_assignment: None,
                initial_submit: vec![],
                workdir_attachments: vec![],
                feature_connections: Default::default(),
                control_operation_id: Some(operation.into()),
                authorization: WorkerCreationAuthorization::Assignment { assignment },
            },
            None,
            Some(&context),
        )
        .await
        .unwrap()
        .0
    }

    fn configure_worker_default(api: &WorkspaceApi, operation: &str, permissions: &str) {
        use crate::config_adoption::{ConfigurationAdoptionRequest, catalog_digest};
        use crate::config_source::ConfigCommitRequest;
        use config_source::{ConfigTreeChange, VirtualPath};
        let owner = test_owner_actor();
        let current = api
            .config_store
            .configuration_current(TEST_WORKSPACE_ID, &owner.account_id)
            .unwrap();
        let main = VirtualPath::parse("main.dcdl").unwrap();
        let registry = api.permission_registry.read().unwrap();
        api.config_store
            .adopt_configuration(
                TEST_WORKSPACE_ID,
                &owner.account_id,
                &ConfigurationAdoptionRequest {
                    operation_id: operation.into(),
                    expected_head: current.history_head,
                    expected_catalog_digest: catalog_digest(registry.host()).unwrap(),
                    expected_legacy_export: None,
                    source: ConfigCommitRequest {
                        base_digest: current.state.snapshot.digest.clone(),
                        changes: vec![ConfigTreeChange::Update {
                            path: main.clone(),
                            expected_digest: current.state.snapshot.entries[&main]
                                .content_digest
                                .clone(),
                            content: format!(
                                "{{ permissions = {permissions}; }} as WorkspaceConfigSchema"
                            ),
                        }],
                        entrypoints: current.state.contract.entrypoints,
                    },
                },
                api.config_schema_registry.compose().unwrap(),
                registry.host(),
                None,
                |_| panic!("Worker-only source has no Subject references"),
                |_| panic!("ordinary settings edit must not transfer authority"),
            )
            .unwrap();
    }

    fn worker_operation(
        api: &WorkspaceApi,
        created: &BrowserCreateWorkerResponse,
        operation: &str,
    ) -> ApiResult<()> {
        subject_authorization::authorize(
            api,
            &RuntimeWorkerRef::new(&created.runtime_id, &created.worker_id),
            &OperationAuthorizationRequest {
                operation: operation.into(),
                resource: PermissionResource::Workspace {
                    workspace_id: TEST_WORKSPACE_ID.into(),
                },
            },
        )
    }

    fn ticket_read(api: &WorkspaceApi, created: &BrowserCreateWorkerResponse) -> ApiResult<()> {
        worker_operation(api, created, "ticket.read")
    }

    #[tokio::test]
    async fn fenced_or_unmappable_subject_does_not_gate_authenticated_account_creation() {
        for fenced in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let config = test_server_config(dir.path());
            let store =
                crate::SqliteWorkspaceStore::open_api_fixture(&config.database_path).unwrap();
            seed_test_registered_workspace(&store, &config)
                .await
                .unwrap();
            let manager =
                crate::FeatureStorage::for_server_database(&config.database_path).unwrap();
            let scope = manager.workspace(TEST_WORKSPACE_ID).unwrap();
            let registration = crate::subjektiv::SubjektivStore::register(&scope).unwrap();
            store
                .adopt_workspace_feature_storage(&scope, &registration, "store_scope")
                .unwrap();
            let legacy = crate::subjektiv::SubjektivStore::open(&scope, &registration).unwrap();
            let subject = legacy
                .create_subject(
                    crate::subjektiv::SubjectRole::new("legacy migration fixture").unwrap(),
                )
                .unwrap();
            if fenced {
                legacy
                    .begin_subject_deletion(&subject.id, "deletion-before-startup")
                    .unwrap();
            } else {
                let current = legacy.current_subject_policy(&subject.id).unwrap().unwrap();
                legacy
                    .set_subject_permissions(
                        &subject.id,
                        &server_api::authorization::SubjectPermissionsUpdate {
                            operation_id: "unsupported-legacy-policy".into(),
                            expected_change_id: Some(current.change_id),
                            execution: vec![server_api::authorization::ScopedPermission {
                                operation: "file.unknown".into(),
                                resource: PermissionResource::Workspace {
                                    workspace_id: TEST_WORKSPACE_ID.into(),
                                },
                            }],
                            delegation: vec![],
                        },
                    )
                    .unwrap();
            }
            let identity = legacy.database_identity().unwrap();
            drop(store);
            let api = WorkspaceApi::new_api_fixture(
                config,
                Arc::new(DeterministicExecutionBackend::default()),
            )
            .await
            .unwrap();
            assert_eq!(
                api.config_store
                    .configuration_policy_is_canonical(TEST_WORKSPACE_ID)
                    .unwrap(),
                fenced
            );
            let receipt = legacy.permission_transfer_receipt(&identity).unwrap();
            assert_eq!(receipt.is_some(), fenced);
            if let Some(receipt) = &receipt {
                assert!(receipt.snapshot.sources.is_empty());
                assert!(
                    api.config_store
                        .adopted_subject_policy(TEST_WORKSPACE_ID, &subject.id)
                        .unwrap()
                        .is_none()
                );
                assert!(matches!(
                    legacy.current_subject_policy(&subject.id),
                    Err(crate::subjektiv::SubjektivError::SubjectNotFound(_))
                ));
            }
            register_test_runtime(&api, WorkdirlessFixtureRuntime::RUNTIME_ID).await;
            api.runtime
                .register_or_replace(WorkdirlessFixtureRuntime::default());
            let created = create_assignment(&api, "independent-account", None).await;
            assert!(worker_operation(&api, &created, "workdir.list").is_ok());
            assert_eq!(
                api.config_store
                    .configuration_policy_is_canonical(TEST_WORKSPACE_ID)
                    .unwrap(),
                fenced
            );
            assert_eq!(
                legacy.permission_transfer_receipt(&identity).unwrap(),
                receipt
            );
        }
    }

    #[tokio::test]
    async fn canonical_startup_rejects_ambiguous_retired_set_without_mutating_authority() {
        use config_source::{ConfigTreeChange, SnapshotEnvironment, VirtualPath};

        let dir = tempfile::tempdir().unwrap();
        let api = test_api(dir.path()).await;
        let config = test_server_config(dir.path());
        let database_path = config.database_path.clone();
        let store = api.config_store.clone();
        let owner = test_owner_actor();
        let initial = store
            .configuration_current(TEST_WORKSPACE_ID, &owner.account_id)
            .unwrap();
        assert!(initial.canonical);
        assert!(initial.policies.is_empty());
        let native = crate::subjektiv::SubjektivStore::open(
            &api.feature_storage,
            &api.subjektiv_registration,
        )
        .unwrap();
        let identity = native.database_identity().unwrap();
        let seal = native
            .permission_transfer_receipt(&identity)
            .unwrap()
            .unwrap();
        assert!(seal.snapshot.sources.is_empty());

        let main = VirtualPath::parse("main.dcdl").unwrap();
        let mut state = initial.state.clone();
        state.snapshot = state
            .snapshot
            .apply(&[ConfigTreeChange::Update {
                path: main.clone(),
                expected_digest: state.snapshot.entries[&main].content_digest.clone(),
                content: r#"{ permissions = {
                    sets = [{ id = "subject-default"; namespaces = [{ namespace = "ticket"; keys = ["ticket.read"]; }]; }];
                    worker_default = { sets = ["subject-default"]; };
                }; } as WorkspaceConfigSchema"#
                    .into(),
            }])
            .unwrap();
        state.projection_digest = SnapshotEnvironment::new(state.snapshot.clone())
            .evaluate_contract(&state.contract)
            .unwrap()
            .projection_digest;
        store
            .with_immediate_transaction(|tx| {
                tx.execute(
                    "DELETE FROM workspace_config_entries WHERE workspace_id=?1",
                    [TEST_WORKSPACE_ID],
                )?;
                crate::config_source::insert_materialized_state(
                    tx,
                    TEST_WORKSPACE_ID,
                    &state,
                    &chrono::Utc::now().to_rfc3339(),
                )?;
                Ok(())
            })
            .unwrap();
        let before = store
            .configuration_current(TEST_WORKSPACE_ID, &owner.account_id)
            .unwrap();
        assert_eq!(before.state, state);
        assert_eq!(before.history_head, initial.history_head);
        assert_eq!(before.policies, initial.policies);
        assert!(before.canonical);
        drop(api);

        let error = WorkspaceApi::new_api_fixture(
            config,
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .err()
        .expect("canonical startup must reject an ambiguous retired Workspace Set");
        assert!(
            error
                .to_string()
                .contains("retired Subject set ID is reserved"),
            "{error}"
        );
        let after = store
            .configuration_current(TEST_WORKSPACE_ID, &owner.account_id)
            .unwrap();
        assert_eq!(after.state, before.state);
        assert_eq!(after.policies, before.policies);
        assert_eq!(after.history_head, before.history_head);
        assert!(after.canonical);
        let manager = crate::FeatureStorage::for_server_database(&database_path).unwrap();
        let scope = manager.workspace(TEST_WORKSPACE_ID).unwrap();
        let registration = crate::subjektiv::SubjektivStore::register(&scope).unwrap();
        let native = crate::subjektiv::SubjektivStore::open(&scope, &registration).unwrap();
        assert_eq!(
            native.permission_transfer_receipt(&identity).unwrap(),
            Some(seal)
        );
    }

    #[tokio::test]
    async fn omitted_assignment_selects_implementer_but_explicit_empty_denies_workdir_list() {
        let dir = tempfile::tempdir().unwrap();
        let api = test_api(dir.path()).await;
        register_test_runtime(&api, WorkdirlessFixtureRuntime::RUNTIME_ID).await;
        api.runtime
            .register_or_replace(WorkdirlessFixtureRuntime::default());
        let default = create_assignment(&api, "default-implementer", None).await;
        let empty = create_assignment(&api, "explicit-empty", Some(Default::default())).await;
        assert!(worker_operation(&api, &default, "workdir.list").is_ok());
        assert!(worker_operation(&api, &empty, "workdir.list").is_err());
        for (created, explicit_empty) in [(&default, false), (&empty, true)] {
            let grant = api
                .config_store
                .core_worker_execution_grant(
                    TEST_WORKSPACE_ID,
                    &created.runtime_id,
                    &created.worker_id,
                )
                .unwrap()
                .unwrap();
            assert_eq!(grant.assignment == Default::default(), explicit_empty);
            assert_eq!(grant.profile, "builtin:companion");
            assert!(matches!(
                grant.source,
                server_api::authorization::CoreWorkerGrantSource::Account { .. }
            ));
        }
    }

    #[tokio::test]
    async fn policy_workspace_set_issuance_freezes_ceiling_and_rechecks_current_membership() {
        use feature_permissions::{
            NamespaceId, NamespacePermissions, PermissionAssignment, PermissionKey,
            PermissionReference, PermissionSetId,
        };
        use server_api::authorization::CoreWorkerExecutionGrant;
        use server_api::launch_policy::{WorkerLaunchPolicyAction, WorkerLaunchPolicyMutation};

        let dir = tempfile::tempdir().unwrap();
        let api = test_api(dir.path()).await;
        register_test_runtime(&api, WorkdirlessFixtureRuntime::RUNTIME_ID).await;
        api.runtime
            .register_or_replace(WorkdirlessFixtureRuntime::default());
        configure_worker_default(
            &api,
            "configure-policy-set",
            r#"{
            sets = [{ id = "policy-reader"; namespaces = [{ namespace = "ticket"; keys = ["ticket.read"]; }]; }];
        }"#,
        );
        let assignment = PermissionAssignment {
            grants: vec![PermissionReference::Set(
                PermissionSetId::parse("policy-reader").unwrap(),
            )],
            ..Default::default()
        };
        let policy = api
            .config_store
            .mutate_worker_launch_policy(
                TEST_WORKSPACE_ID,
                &test_owner_actor().account_id,
                &WorkerLaunchPolicyMutation {
                    operation_key: "create-reader-policy".into(),
                    policy_id: "reader".into(),
                    expected: None,
                    action: WorkerLaunchPolicyAction::Create {
                        name: "Workspace reader".into(),
                        allowed_profiles: vec!["builtin:companion".into()],
                        child_permissions: assignment.clone(),
                    },
                },
            )
            .unwrap()
            .policy;
        let usage = policy.usage_key().unwrap();
        let launch_assignment = |keys: &[&str]| PermissionAssignment {
            grants: vec![PermissionReference::Namespace(NamespacePermissions {
                namespace: NamespaceId::parse("worker").unwrap(),
                keys: keys
                    .iter()
                    .map(|key| PermissionKey::parse(*key).unwrap())
                    .collect(),
            })],
            ..Default::default()
        };
        let prepare = |source: &RuntimeWorkerRef, operation: &str| {
            subject_authorization::prepare_core_policy_grant(
                &api,
                source,
                &policy.policy_id,
                policy.authorization_revision,
                WorkdirlessFixtureRuntime::RUNTIME_ID,
                "builtin:companion",
                "fixture-profile-digest",
                operation,
            )
        };
        assert!(
            prepare(
                &RuntimeWorkerRef::new(WorkdirlessFixtureRuntime::RUNTIME_ID, "unbound"),
                "unbound-origin",
            )
            .is_err()
        );
        for (operation, keys) in [
            ("missing-policy-usage", vec!["worker.create"]),
            ("missing-worker-create", vec![usage.as_str()]),
        ] {
            let parent = create_assignment(&api, operation, Some(launch_assignment(&keys))).await;
            assert!(
                prepare(
                    &RuntimeWorkerRef::new(&parent.runtime_id, &parent.worker_id),
                    operation,
                )
                .is_err()
            );
        }
        let parent = create_assignment(
            &api,
            "authorized-launch-parent",
            Some(launch_assignment(&["worker.create", usage.as_str()])),
        )
        .await;
        let source = RuntimeWorkerRef::new(&parent.runtime_id, &parent.worker_id);
        let read = PermissionKey::parse("ticket.read").unwrap();
        let query = PermissionKey::parse("ticket.query").unwrap();
        let mut issued = prepare(&source, "issue-reader").unwrap();
        assert_eq!(issued.assignment, assignment);
        assert_eq!(issued.issuance_ceiling, Some(vec![read.clone()]));
        issued.worker_id = "policy-child".into();
        api.config_store
            .bind_core_worker_execution_grant(&issued)
            .unwrap();
        let allowed = |grant: &CoreWorkerExecutionGrant, operation: &str| {
            subject_authorization::authorize(
                &api,
                &RuntimeWorkerRef::new(&grant.runtime_id, &grant.worker_id),
                &OperationAuthorizationRequest {
                    operation: operation.into(),
                    resource: PermissionResource::Workspace {
                        workspace_id: TEST_WORKSPACE_ID.into(),
                    },
                },
            )
        };
        assert!(allowed(&issued, "ticket.read").is_ok());
        assert!(allowed(&issued, "ticket.query").is_err());

        configure_worker_default(
            &api,
            "grow-policy-set",
            r#"{
            sets = [{ id = "policy-reader"; namespaces = [{ namespace = "ticket"; keys = ["ticket.read", "ticket.query"]; }]; }];
        }"#,
        );
        assert_eq!(
            api.config_store
                .worker_launch_policy(TEST_WORKSPACE_ID, &policy.policy_id)
                .unwrap(),
            Some(policy.clone())
        );
        assert!(allowed(&issued, "ticket.read").is_ok());
        assert!(allowed(&issued, "ticket.query").is_err());
        let mut expanded = prepare(&source, "issue-expanded-reader").unwrap();
        assert_eq!(expanded.assignment, assignment);
        assert_eq!(
            expanded.issuance_ceiling,
            Some(vec![query.clone(), read.clone()])
        );
        expanded.worker_id = "expanded-policy-child".into();
        api.config_store
            .bind_core_worker_execution_grant(&expanded)
            .unwrap();
        assert!(allowed(&expanded, "ticket.read").is_ok());
        assert!(allowed(&expanded, "ticket.query").is_ok());

        configure_worker_default(
            &api,
            "shrink-policy-set",
            r#"{
            sets = [{ id = "policy-reader"; namespaces = [{ namespace = "ticket"; keys = ["ticket.query"]; }]; }];
        }"#,
        );
        assert!(allowed(&issued, "ticket.read").is_err());
        assert!(allowed(&issued, "ticket.query").is_err());
        assert!(allowed(&expanded, "ticket.read").is_err());
        assert!(allowed(&expanded, "ticket.query").is_ok());
        for grant in [&issued, &expanded] {
            assert_eq!(
                api.config_store
                    .core_worker_execution_grant(
                        TEST_WORKSPACE_ID,
                        &grant.runtime_id,
                        &grant.worker_id,
                    )
                    .unwrap()
                    .as_ref(),
                Some(grant)
            );
        }
        assert_eq!(
            api.config_store
                .worker_launch_policy(TEST_WORKSPACE_ID, &policy.policy_id)
                .unwrap(),
            Some(policy)
        );
    }

    #[tokio::test]
    async fn assignment_replay_preserves_saved_grant_after_worker_default_changes() {
        let dir = tempfile::tempdir().unwrap();
        let api = test_api(dir.path()).await;
        register_test_runtime(&api, WorkdirlessFixtureRuntime::RUNTIME_ID).await;
        api.runtime
            .register_or_replace(WorkdirlessFixtureRuntime::default());
        let original = create_assignment(&api, "replay-default", None).await;
        let saved = api
            .config_store
            .core_worker_execution_grant(
                TEST_WORKSPACE_ID,
                &original.runtime_id,
                &original.worker_id,
            )
            .unwrap()
            .unwrap();
        assert!(worker_operation(&api, &original, "workdir.list").is_ok());
        configure_worker_default(&api, "replace-default", "{ worker_default = {}; }");
        let replayed = create_assignment(&api, "replay-default", None).await;
        assert_eq!(replayed.worker_id, original.worker_id);
        assert_eq!(
            api.config_store
                .core_worker_execution_grant(
                    TEST_WORKSPACE_ID,
                    &replayed.runtime_id,
                    &replayed.worker_id,
                )
                .unwrap(),
            Some(saved.clone())
        );
        subject_authorization::revalidate_core_grant(&api, &saved).unwrap();
        assert!(worker_operation(&api, &replayed, "workdir.list").is_ok());
        let newly_created = create_assignment(&api, "new-empty-default", None).await;
        assert!(worker_operation(&api, &newly_created, "workdir.list").is_err());
    }

    #[tokio::test]
    async fn worker_assignment_default_is_bounded_and_live_source_revocation_denies_children() {
        use feature_permissions::{PermissionAssignment, PermissionReference, PermissionSetId};
        use server_api::authorization::{CoreWorkerGrantParent, CoreWorkerGrantSource};

        let dir = tempfile::tempdir().unwrap();
        let api = test_api(dir.path()).await;
        register_test_runtime(&api, WorkdirlessFixtureRuntime::RUNTIME_ID).await;
        api.runtime
            .register_or_replace(WorkdirlessFixtureRuntime::default());
        configure_worker_default(
            &api,
            "source-ceiling",
            r#"{
            sets = [{ id = "issuer"; namespaces = [
                { namespace = "worker"; keys = ["worker.create"]; },
                { namespace = "workdir"; keys = ["workdir.list"]; }
            ]; }];
        }"#,
        );
        let parent = create_assignment(
            &api,
            "assignment-parent",
            Some(PermissionAssignment {
                grants: vec![PermissionReference::Set(
                    PermissionSetId::parse("issuer").unwrap(),
                )],
                ..Default::default()
            }),
        )
        .await;
        let source = RuntimeWorkerRef::new(&parent.runtime_id, &parent.worker_id);
        let prepare = |operation: &str, explicit: Option<&PermissionAssignment>| {
            subject_authorization::WorkerAuthorityView::prepare_assignment_grant(
                &api,
                &source,
                explicit,
                &parent.runtime_id,
                "builtin:companion",
                "fixture-profile-digest",
                operation,
            )
        };
        let mut child = prepare("bounded-assignment", None).unwrap();
        assert_eq!(
            child.assignment,
            PermissionAssignment {
                grants: vec![PermissionReference::Set(
                    PermissionSetId::parse("implementer").unwrap()
                )],
                ..Default::default()
            }
        );
        let account_grant = api
            .config_store
            .core_worker_execution_grant(TEST_WORKSPACE_ID, &parent.runtime_id, &parent.worker_id)
            .unwrap()
            .unwrap();
        assert!(matches!(&child.source, CoreWorkerGrantSource::Delegation {
            parent: CoreWorkerGrantParent::WorkerGrant { grant_id, .. }
        } if grant_id == &account_grant.grant_id));
        let ceiling = child.issuance_ceiling.as_ref().unwrap();
        assert!(
            ceiling.contains(&feature_permissions::PermissionKey::parse("workdir.list").unwrap())
        );
        assert!(
            ceiling.contains(&feature_permissions::PermissionKey::parse("worker.create").unwrap())
        );
        assert!(
            !ceiling.contains(&feature_permissions::PermissionKey::parse("ticket.read").unwrap())
        );
        child.worker_id = "bounded-assignment-child".into();
        api.config_store
            .bind_core_worker_execution_grant(&child)
            .unwrap();
        let allowed = |operation: &str| {
            subject_authorization::authorize(
                &api,
                &RuntimeWorkerRef::new(&child.runtime_id, &child.worker_id),
                &OperationAuthorizationRequest {
                    operation: operation.into(),
                    resource: PermissionResource::Workspace {
                        workspace_id: TEST_WORKSPACE_ID.into(),
                    },
                },
            )
        };
        assert!(allowed("workdir.list").is_ok());
        assert!(allowed("ticket.read").is_err());
        let explicit_reader = PermissionAssignment {
            grants: vec![PermissionReference::Namespace(
                feature_permissions::NamespacePermissions {
                    namespace: feature_permissions::NamespaceId::parse("ticket").unwrap(),
                    keys: vec![feature_permissions::PermissionKey::parse("ticket.read").unwrap()],
                },
            )],
            ..Default::default()
        };
        assert!(prepare("out-of-bound-assignment", Some(&explicit_reader)).is_err());
        let mut empty = prepare("empty-assignment", Some(&Default::default())).unwrap();
        assert_eq!(empty.assignment, Default::default());
        assert_eq!(empty.issuance_ceiling, Some(vec![]));
        empty.worker_id = "empty-assignment-child".into();
        api.config_store
            .bind_core_worker_execution_grant(&empty)
            .unwrap();
        assert!(
            subject_authorization::authorize(
                &api,
                &RuntimeWorkerRef::new(&empty.runtime_id, &empty.worker_id),
                &OperationAuthorizationRequest {
                    operation: "workdir.list".into(),
                    resource: PermissionResource::Workspace {
                        workspace_id: TEST_WORKSPACE_ID.into()
                    },
                },
            )
            .is_err()
        );

        configure_worker_default(
            &api,
            "grow-parent-ceiling",
            r#"{
            sets = [{ id = "issuer"; namespaces = [
                { namespace = "worker"; keys = ["worker.create"]; },
                { namespace = "workdir"; keys = ["workdir.list"]; },
                { namespace = "ticket"; keys = ["ticket.read"]; }
            ]; }];
        }"#,
        );
        assert!(ticket_read(&api, &parent).is_ok());
        assert!(allowed("ticket.read").is_err());
        assert!(allowed("workdir.list").is_ok());
        assert!(
            subject_authorization::WorkerAuthorityView::prepare_assignment_grant(
                &api,
                &RuntimeWorkerRef::new(&child.runtime_id, &child.worker_id),
                Some(&explicit_reader),
                &child.runtime_id,
                "builtin:companion",
                "fixture-profile-digest",
                "grandchild-cannot-bypass-parent-ceiling",
            )
            .is_err()
        );
        configure_worker_default(
            &api,
            "shrink-parent-ceiling",
            r#"{
            sets = [{ id = "issuer"; namespaces = [
                { namespace = "worker"; keys = ["worker.create"]; }
            ]; }];
        }"#,
        );
        assert!(allowed("workdir.list").is_err());
        subject_authorization::revalidate_core_grant(&api, &child).unwrap();
        configure_worker_default(
            &api,
            "remove-parent-create",
            r#"{
            sets = [{ id = "issuer"; namespaces = [
                { namespace = "workdir"; keys = ["workdir.list"]; }
            ]; }];
        }"#,
        );
        assert!(prepare("source-lost-create", None).is_err());
        assert!(subject_authorization::revalidate_core_grant(&api, &child).is_err());
        assert!(allowed("workdir.list").is_err());
        configure_worker_default(
            &api,
            "restore-parent-keys",
            r#"{
            sets = [{ id = "issuer"; namespaces = [
                { namespace = "worker"; keys = ["worker.create"]; },
                { namespace = "workdir"; keys = ["workdir.list"]; }
            ]; }];
        }"#,
        );
        assert!(allowed("workdir.list").is_ok());
        subject_authorization::revalidate_core_grant(&api, &empty).unwrap();
        api.config_store
            .revoke_core_worker_execution_grant(
                TEST_WORKSPACE_ID,
                &test_owner_actor().account_id,
                &account_grant.grant_id,
                "revoke-assignment-source",
            )
            .unwrap();
        assert!(subject_authorization::revalidate_core_grant(&api, &child).is_err());
        assert!(subject_authorization::revalidate_core_grant(&api, &empty).is_err());
        assert!(prepare("revoked-assignment-source", None).is_err());
        assert_eq!(api.config_store.core_worker_execution_grant(
            TEST_WORKSPACE_ID, &child.runtime_id, &child.worker_id,
        ).unwrap(), Some(child));
    }

    #[tokio::test]
    async fn workspace_set_is_checked_again_at_operation_time_without_profile_authority() {
        let dir = tempfile::tempdir().unwrap();
        let api = test_api(dir.path()).await;
        register_test_runtime(&api, WorkdirlessFixtureRuntime::RUNTIME_ID).await;
        api.runtime
            .register_or_replace(WorkdirlessFixtureRuntime::default());
        configure_worker_default(
            &api,
            "set-default",
            r#"{
            sets = [{ id = "reader"; namespaces = [{ namespace = "ticket"; keys = ["ticket.read"]; }]; }];
            worker_default = { sets = ["reader"]; };
        }"#,
        );
        let created = create_assignment(&api, "selected-reader", None).await;
        let empty = create_assignment(&api, "selected-empty", Some(Default::default())).await;
        assert!(ticket_read(&api, &created).is_ok());
        assert!(ticket_read(&api, &empty).is_err());
        let before = api
            .config_store
            .core_worker_execution_grant(TEST_WORKSPACE_ID, &created.runtime_id, &created.worker_id)
            .unwrap()
            .unwrap();
        configure_worker_default(
            &api,
            "exclude-current-set",
            r#"{
            sets = [{ id = "reader"; }];
            worker_default = {};
        }"#,
        );
        assert!(ticket_read(&api, &created).is_err());
        assert_eq!(
            api.config_store
                .core_worker_execution_grant(
                    TEST_WORKSPACE_ID,
                    &created.runtime_id,
                    &created.worker_id,
                )
                .unwrap(),
            Some(before)
        );
    }
}
