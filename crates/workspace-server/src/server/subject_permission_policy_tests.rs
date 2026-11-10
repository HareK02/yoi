mod subject_permission_policy_tests {
    //! HTTP/account authentication, native lifecycle fencing and canonical policy
    //! commits use isolated real stores in the normal Server library tests. Only
    //! Runtime execution is replaced by the shared workdirless fixture.

    use super::*;
    use feature_permissions::{PermissionAssignment, PermissionReference, PermissionSetId};
    use server_api::authorization::{OperationAuthorizationRequest, PermissionResource};
    use server_api::subject_policy::{
        SubjectPermissionPolicyAction, SubjectPermissionPolicyMutation,
        SubjectPermissionPolicyResponse,
    };

    struct PolicyFixture {
        api: WorkspaceApi,
        owner: String,
        subject: String,
        creation: Value,
        _dir: tempfile::TempDir,
    }

    impl PolicyFixture {
        async fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let api = test_api(dir.path()).await;
            let owner = seed_test_workspace_owner_token(api.store.as_ref(), "policy-owner").await;
            let creation = json!({
                "operation_id": "policy-subject-creation",
                "role": "permission-policy",
                "behavior_md": ""
            });
            let (status, body) = send(
                &api,
                human(&owner, "POST", &subjects_path(), Some(creation.clone())),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED, "{body}");
            let subject = body["id"].as_str().unwrap().to_owned();
            Self {
                api,
                owner,
                subject,
                creation,
                _dir: dir,
            }
        }

        fn path(&self) -> String {
            format!("{}/{}/permission-policy", subjects_path(), self.subject)
        }

        async fn read(&self) -> Value {
            let (status, body) =
                send(&self.api, human(&self.owner, "GET", &self.path(), None)).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            let projection: SubjectPermissionPolicyResponse =
                serde_json::from_value(body.clone()).unwrap();
            assert_eq!(projection.catalog.workspace.as_str(), TEST_WORKSPACE_ID);
            assert_eq!(
                projection.execution_resolution.workspace,
                projection.catalog.workspace
            );
            assert_eq!(
                projection.delegation_resolution.workspace,
                projection.catalog.workspace
            );
            if let Some(policy) = &projection.policy {
                assert_eq!(projection.change_id.as_ref(), Some(&policy.change_id));
            }
            body
        }

        async fn put(&self, request: Value, expected: StatusCode) -> Value {
            let (status, body) = send(
                &self.api,
                human(&self.owner, "PUT", &self.path(), Some(request)),
            )
            .await;
            assert_eq!(status, expected, "{body}");
            body
        }

        fn assert_current(&self, policy: &Value) {
            let current = self
                .api
                .config_store
                .current_subject_permission_policy(TEST_WORKSPACE_ID, &self.subject)
                .unwrap();
            assert_eq!(serde_json::to_value(current).unwrap(), *policy);
            let native = open_subjektiv_store(&self.api).unwrap();
            assert_eq!(
                serde_json::to_value(native.current_subject_policy(&self.subject).unwrap())
                    .unwrap(),
                *policy
            );
        }

        async fn worker(&self, connected: bool) -> RuntimeWorkerRef {
            if connected {
                super::subject_authorization_tests::install_read_only_subject_profile(&self.api);
            }
            register_test_runtime(&self.api, WorkdirlessFixtureRuntime::RUNTIME_ID).await;
            self.api
                .runtime
                .register_or_replace(WorkdirlessFixtureRuntime::default());
            let context = server_request_context(
                Some(test_owner_actor()),
                None,
                None,
                None,
                &HeaderMap::new(),
            );
            let Json(worker) = create_workspace_worker_authenticated(
                self.api.clone(),
                HeaderMap::new(),
                CreateWorkspaceWorkerRequest {
                    runtime_id: WorkdirlessFixtureRuntime::RUNTIME_ID.into(),
                    display_name: "policy-test-worker".into(),
                    singleton_key: None,
                    profile: Some(if connected {
                        "project:authority-source".into()
                    } else {
                        "builtin:companion".into()
                    }),
                    ticket_assignment: None,
                    initial_submit: vec![],
                    workdir_attachments: vec![],
                    feature_connections: server_api::WorkspaceWorkerFeatureConnectionsRequest {
                        subjektiv: connected.then(|| {
                            server_api::SubjektivWorkerConnectionRequest {
                                subject_id: self.subject.clone(),
                            }
                        }),
                    },
                    control_operation_id: Some("policy-test-worker".into()),
                    authorization:
                        server_api::launch_policy::WorkerCreationAuthorization::Assignment {
                            assignment: None,
                        },
                },
                None,
                Some(&context),
            )
            .await
            .unwrap();
            RuntimeWorkerRef::new(worker.runtime_id, worker.worker_id)
        }
    }

    fn subjects_path() -> String {
        format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects")
    }

    fn human(token: &str, method: &str, path: &str, body: Option<Value>) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", format!("Bearer {token}"))
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                body.map(|body| body.to_string()).unwrap_or_default(),
            ))
            .unwrap()
    }

    async fn send(api: &WorkspaceApi, request: Request<Body>) -> (StatusCode, Value) {
        let response = build_router(api.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        let body = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| json!({"text": String::from_utf8_lossy(&bytes)}));
        (status, body)
    }

    fn replace(screen: &Value, operation: &str, execution: PermissionAssignment) -> Value {
        serde_json::to_value(SubjectPermissionPolicyMutation {
            operation_id: operation.into(),
            expected_change_id: screen["change_id"].as_str().map(str::to_owned),
            action: SubjectPermissionPolicyAction::Replace {
                execution,
                delegation: PermissionAssignment::default(),
            },
        })
        .unwrap()
    }

    fn remove(screen: &Value, operation: &str) -> Value {
        json!({
            "operation_id": operation,
            "expected_change_id": screen["change_id"],
            "action": {"kind": "remove"}
        })
    }

    fn configure_shared_set(f: &PolicyFixture, operation: &str, key: &str) {
        use crate::config_adoption::{ConfigurationAdoptionRequest, catalog_digest};
        use crate::config_source::ConfigCommitRequest;
        use config_source::{ConfigTreeChange, VirtualPath};

        let account = test_owner_actor().account_id;
        let current = f
            .api
            .config_store
            .configuration_current(TEST_WORKSPACE_ID, &account)
            .unwrap();
        let main = VirtualPath::parse("main.dcdl").unwrap();
        let registry = f.api.permission_registry.read().unwrap();
        f.api
            .config_store
            .adopt_configuration(
                TEST_WORKSPACE_ID,
                &account,
                &ConfigurationAdoptionRequest {
                    operation_id: operation.into(),
                    expected_head: current.history_head,
                    expected_catalog_digest: catalog_digest(registry.host()).unwrap(),
                    expected_legacy_export: None,
                    source: ConfigCommitRequest {
                        base_digest: current.state.snapshot.digest.clone(),
                        entrypoints: current.state.contract.entrypoints,
                        changes: vec![ConfigTreeChange::Update {
                            path: main.clone(),
                            expected_digest: current.state.snapshot.entries[&main]
                                .content_digest
                                .clone(),
                            content: format!(
                                r#"{{ permissions = {{ sets = [{{ id = "policy-audit";
                                    namespaces = [{{ namespace = "ticket"; keys = ["{key}"]; }}];
                                }}]; }}; }} as WorkspaceConfigSchema"#
                            ),
                        }],
                    },
                },
                f.api.config_schema_registry.compose().unwrap(),
                registry.host(),
                None,
                |_| panic!("shared sets do not refer to individual Subjects"),
                |_| panic!("shared settings edits do not migrate Subject authority"),
            )
            .unwrap();
    }

    #[tokio::test]
    async fn dedicated_get_resolves_current_shared_sets_without_rewriting_subject_policy() {
        let f = PolicyFixture::new().await;
        configure_shared_set(&f, "configure-audit-read", "ticket.read");
        let initial = f.read().await;
        let saved = f
            .put(
                replace(
                    &initial,
                    "select-shared-audit-set",
                    PermissionAssignment {
                        grants: vec![PermissionReference::Set(
                            PermissionSetId::parse("policy-audit").unwrap(),
                        )],
                        ..Default::default()
                    },
                ),
                StatusCode::OK,
            )
            .await;
        let first = f.read().await;
        assert_eq!(first["policy"], saved["policy"]);
        assert_eq!(
            first["execution_resolution"]["allowed"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["ticket.read"]
        );
        assert_eq!(first["execution_resolution"]["diagnostics"], json!([]));
        assert_eq!(first["delegation_resolution"]["allowed"], json!({}));
        configure_shared_set(&f, "configure-audit-query", "ticket.query");
        let before = f
            .api
            .config_store
            .configuration_current(TEST_WORKSPACE_ID, &test_owner_actor().account_id)
            .unwrap();
        let second = f.read().await;
        assert_eq!(second["change_id"], first["change_id"]);
        assert_eq!(second["policy"], first["policy"]);
        assert_eq!(
            second["execution_resolution"]["allowed"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["ticket.query"]
        );
        assert_eq!(second["execution_resolution"]["diagnostics"], json!([]));
        let after = f
            .api
            .config_store
            .configuration_current(TEST_WORKSPACE_ID, &test_owner_actor().account_id)
            .unwrap();
        assert_eq!(after.state, before.state);
        assert_eq!(after.history_head, before.history_head);
        f.assert_current(&saved["policy"]);
    }

    #[tokio::test]
    async fn get_keeps_legacy_domain_constraints_separate_from_reference_resolution() {
        use crate::policy_config::AdoptedSubjectPolicy;
        use server_api::authorization::{
            CurrentSubjectPolicy, ScopedPermission, SubjectPermissions,
        };

        let f = PolicyFixture::new().await;
        let screen = f.read().await;
        let mut policy: CurrentSubjectPolicy =
            serde_json::from_value(screen["policy"].clone()).unwrap();
        policy.legacy_constraints = Some(SubjectPermissions {
            change_id: "retained-legacy-constraints".into(),
            execution: vec![ScopedPermission {
                operation: "ticket.read".into(),
                resource: PermissionResource::Workspace {
                    workspace_id: TEST_WORKSPACE_ID.into(),
                },
            }],
            delegation: vec![],
        });
        let stored =
            serde_json::to_string(&AdoptedSubjectPolicy::from_current(&f.subject, &policy))
                .unwrap();
        // Model a migrated constrained row, not a new owner-issued assignment.
        f.api
            .config_store
            .with_immediate_transaction(|transaction| {
                transaction.execute(
                    "UPDATE workspace_configuration_policies SET policy_json=?1 WHERE workspace_id=?2 AND subject_id=?3",
                    rusqlite::params![stored, TEST_WORKSPACE_ID, f.subject],
                )?;
                Ok(())
            })
            .unwrap();
        let projected = f.read().await;
        assert_eq!(projected["policy"], serde_json::to_value(&policy).unwrap());
        assert!(projected["execution_resolution"]["allowed"]["ticket.query"].is_array());
        assert_eq!(projected["execution_resolution"]["diagnostics"], json!([]));
        f.assert_current(&projected["policy"]);
    }

    #[tokio::test]
    async fn human_reads_report_owner_management_and_nonowner_writes_are_forbidden() {
        let f = PolicyFixture::new().await;
        let owner = f.read().await;
        assert_eq!(owner["can_manage"], true);
        assert!(owner["policy"].is_object());
        let other = seed_test_api_token(f.api.store.as_ref(), "policy-nonowner");
        let (status, screen) = send(&f.api, human(&other, "GET", &f.path(), None)).await;
        assert_eq!(status, StatusCode::OK, "{screen}");
        assert_eq!(screen["can_manage"], false);
        assert_eq!(screen["policy"], owner["policy"]);
        for field in [
            "change_id",
            "catalog",
            "execution_resolution",
            "delegation_resolution",
        ] {
            assert_eq!(screen[field], owner[field], "{field}");
        }
        assert!(
            owner["catalog"]["namespaces"]
                .as_array()
                .unwrap()
                .iter()
                .any(|namespace| namespace["namespace"] == "ticket")
        );
        assert!(owner["execution_resolution"]["allowed"]["ticket.read"].is_array());
        let (status, body) = send(
            &f.api,
            human(
                &other,
                "PUT",
                &f.path(),
                Some(remove(&screen, "nonowner-remove")),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        f.assert_current(&owner["policy"]);
        for method in ["GET", "PUT"] {
            let request = Request::builder()
                .method(method)
                .uri(f.path())
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(remove(&owner, "unauthenticated").to_string()))
                .unwrap();
            let (status, body) = send(&f.api, request).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
        }
    }

    #[tokio::test]
    async fn signed_worker_and_runtime_cannot_read_or_manage_subject_policy() {
        let mut f = PolicyFixture::new().await;
        let worker = f.worker(false).await;
        let identity =
            worker_runtime::auth::RuntimeIdentityMaterial::generate(&worker.runtime_id).unwrap();
        configure_runtime_request_auth(&mut f.api, &identity, &worker.runtime_id);
        let screen = f.read().await;
        for source in [None, Some(worker.worker_id.as_str())] {
            for method in ["GET", "PUT"] {
                let body = if method == "PUT" {
                    serde_json::to_vec(&remove(&screen, "runtime-remove")).unwrap()
                } else {
                    vec![]
                };
                let (status, body) = send(
                    &f.api,
                    runtime_source_request(&identity, source, method, &f.path(), body),
                )
                .await;
                assert_eq!(status, StatusCode::FORBIDDEN, "{method} {source:?}: {body}");
            }
        }
        f.assert_current(&screen["policy"]);
    }

    #[tokio::test]
    async fn stale_cas_and_historical_replay_preserve_current_policy_and_configuration() {
        let f = PolicyFixture::new().await;
        let account = test_owner_actor().account_id;
        let before = f
            .api
            .config_store
            .configuration_current(TEST_WORKSPACE_ID, &account)
            .unwrap();
        let initial = f.read().await;
        let first_request = replace(
            &initial,
            "first-replacement",
            PermissionAssignment::default(),
        );
        let first = f.put(first_request.clone(), StatusCode::OK).await;
        assert_ne!(first["policy"]["change_id"], initial["policy"]["change_id"]);
        f.put(
            replace(&initial, "stale-screen", PermissionAssignment::default()),
            StatusCode::CONFLICT,
        )
        .await;
        let second_request = replace(
            &first,
            "second-replacement",
            PermissionAssignment {
                revoked: true,
                ..Default::default()
            },
        );
        let second = f.put(second_request, StatusCode::OK).await;
        let replay = f.put(first_request.clone(), StatusCode::OK).await;
        assert_eq!(replay, first);
        f.assert_current(&second["policy"]);
        let mut reuse = first_request;
        reuse["action"]["execution"]["revoked"] = json!(true);
        f.put(reuse, StatusCode::CONFLICT).await;
        f.assert_current(&second["policy"]);
        let after = f
            .api
            .config_store
            .configuration_current(TEST_WORKSPACE_ID, &account)
            .unwrap();
        assert_eq!(after.state, before.state);
        assert_eq!(after.history_head, before.history_head);
    }

    #[tokio::test]
    async fn removal_and_creation_retry_do_not_reissue_defaults_or_edit_configuration() {
        let f = PolicyFixture::new().await;
        let account = test_owner_actor().account_id;
        let before = f
            .api
            .config_store
            .configuration_current(TEST_WORKSPACE_ID, &account)
            .unwrap();
        let screen = f.read().await;
        let removal = remove(&screen, "explicit-policy-removal");
        let removed = f.put(removal.clone(), StatusCode::OK).await;
        assert_eq!(removed["policy"], Value::Null);
        assert!(removed["change_id"].is_string());
        assert_ne!(removed["change_id"], screen["change_id"]);
        f.assert_current(&Value::Null);
        let (status, retried) = send(
            &f.api,
            human(&f.owner, "POST", &subjects_path(), Some(f.creation.clone())),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{retried}");
        assert_eq!(retried["id"], f.subject);
        let unset = f.read().await;
        assert_eq!(unset["policy"], Value::Null);
        assert_eq!(unset["change_id"], removed["change_id"]);
        for lane in ["execution_resolution", "delegation_resolution"] {
            assert_eq!(unset[lane]["allowed"], json!({}));
            assert_eq!(unset[lane]["diagnostics"], json!([{ "kind": "unset" }]));
        }
        f.assert_current(&Value::Null);
        let recreated = f
            .put(
                replace(
                    &unset,
                    "explicit-policy-replacement",
                    PermissionAssignment::default(),
                ),
                StatusCode::OK,
            )
            .await;
        assert!(recreated["policy"].is_object());
        assert_eq!(f.put(removal, StatusCode::OK).await, removed);
        f.assert_current(&recreated["policy"]);
        let after = f
            .api
            .config_store
            .configuration_current(TEST_WORKSPACE_ID, &account)
            .unwrap();
        assert_eq!(after.state, before.state);
        assert_eq!(after.history_head, before.history_head);
    }

    #[tokio::test]
    async fn current_revocation_and_removal_fence_an_existing_subject_body() {
        let f = PolicyFixture::new().await;
        let worker = f.worker(true).await;
        let request = OperationAuthorizationRequest {
            operation: "ticket.read".into(),
            resource: PermissionResource::Workspace {
                workspace_id: TEST_WORKSPACE_ID.into(),
            },
        };
        subject_authorization::authorize(&f.api, &worker, &request).unwrap();
        let initial = f.read().await;
        let mut execution: PermissionAssignment =
            serde_json::from_value(initial["policy"]["execution"].clone()).unwrap();
        execution.revoked = true;
        let revoked = f
            .put(
                replace(&initial, "revoke-current-execution", execution),
                StatusCode::OK,
            )
            .await;
        assert!(subject_authorization::authorize(&f.api, &worker, &request).is_err());
        f.put(remove(&revoked, "remove-current-policy"), StatusCode::OK)
            .await;
        assert!(subject_authorization::authorize(&f.api, &worker, &request).is_err());
        f.assert_current(&Value::Null);
    }

    #[tokio::test]
    async fn unknown_references_are_rejected_even_when_revoked_without_changing_current_policy() {
        let f = PolicyFixture::new().await;
        let initial = f.read().await;
        for revoked in [false, true] {
            let invalid = replace(
                &initial,
                if revoked {
                    "invalid-revoked"
                } else {
                    "invalid-active"
                },
                PermissionAssignment {
                    grants: vec![PermissionReference::Set(
                        PermissionSetId::parse("missing-policy-set").unwrap(),
                    )],
                    revoked,
                    ..Default::default()
                },
            );
            f.put(invalid, StatusCode::BAD_REQUEST).await;
            f.assert_current(&initial["policy"]);
        }
    }

    #[tokio::test]
    async fn missing_subject_and_deletion_fence_reject_without_exposing_or_mutating_policy() {
        let f = PolicyFixture::new().await;
        let initial = f.read().await;
        let missing = format!("{}/missing-subject/permission-policy", subjects_path());
        let (status, body) = send(&f.api, human(&f.owner, "GET", &missing, None)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        let (status, body) = send(
            &f.api,
            human(
                &f.owner,
                "PUT",
                &missing,
                Some(remove(&initial, "missing-remove")),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        open_subjektiv_store(&f.api)
            .unwrap()
            .begin_subject_deletion(&f.subject, "policy-deletion-fence")
            .unwrap();
        let (status, body) = send(&f.api, human(&f.owner, "GET", &f.path(), None)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        let (status, body) = send(
            &f.api,
            human(
                &f.owner,
                "PUT",
                &f.path(),
                Some(remove(&initial, "fenced-remove")),
            ),
        )
        .await;
        assert!(!status.is_success(), "{body}");
        let current = f
            .api
            .config_store
            .current_subject_permission_policy(TEST_WORKSPACE_ID, &f.subject)
            .unwrap();
        assert_eq!(serde_json::to_value(current).unwrap(), initial["policy"]);
    }
}
