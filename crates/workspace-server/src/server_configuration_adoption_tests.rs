// Included inside server::tests to reuse its catalog, identity and storage fixtures.
mod configuration_adoption_http_integration {
    //! Ordinary Server library integration tests: real catalog router (including
    //! cookie-origin middleware), owner/session lookup, builtin permission catalog,
    //! migration, guarded Feature storage and Server SQLite adoption. Only unused
    //! Runtime execution is the existing deterministic fixture. Router oneshot is
    //! the HTTP boundary; these are not TCP-loss, OS-crash or store-fault tests.
    //! The separate configuration_transfer/config_adoption suites own store faults.
    use super::*;
    use server_api::authorization::{
        PermissionResource, ScopedPermission, SubjectPermissions, SubjectPermissionsUpdate,
    };
    use server_api::{
        ConfigCommitRequest as SourceRequest, ConfigTreeChange as Change,
        ConfigurationAdoptionReceipt as Receipt, ConfigurationAdoptionRequest as Adoption,
        ConfigurationPreviewResponse as Preview, ConfigurationReconciliation as Reconciliation,
        ConfigurationSourceResponse as Source, RuntimePolicyApplication,
    };

    const OWNER_SUFFIX: &str = "adoption-http-owner";
    const COOKIE_TOKEN: &str = "adoption-http-browser-session";
    const MARKER: &str = "notes/adoption-http.txt";

    struct Fixture {
        api: WorkspaceApi,
        app: Router,
        owner: String,
        foreign: String,
        owner_account: String,
        cookie: String,
        origin: String,
        subject: String,
        legacy: crate::subjektiv::SubjektivStore,
        legacy_update: SubjectPermissionsUpdate,
        legacy_policy: SubjectPermissions,
        runtime_identity: RuntimeIdentityMaterial,
        _dir: tempfile::TempDir,
    }

    impl Fixture {
        async fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let mut api = test_api(dir.path()).await;
            let owner_account = api
                .store
                .get_workspace(TEST_WORKSPACE_ID)
                .await
                .unwrap()
                .unwrap()
                .owner_account_id;
            let owner = seed_test_workspace_owner_token(api.store.as_ref(), OWNER_SUFFIX).await;
            let foreign = seed_test_api_token(api.store.as_ref(), "adoption-http-nonowner");
            api.store
                .create_browser_session(&BrowserSessionRecord {
                    token_hash: crate::auth::token_hash(COOKIE_TOKEN),
                    session_id: "adoption-http-session-id".into(),
                    user_id: format!("user-{OWNER_SUFFIX}"),
                    created_at: "2026-01-01T00:00:00Z".into(),
                    expires_at: "2099-01-01T00:00:00Z".into(),
                    revoked_at: None,
                })
                .unwrap();
            let AuthConfig::Passkey {
                origin,
                cookie_name,
                ..
            } = &api.config.auth;
            let origin = origin.clone();
            let cookie = format!("{cookie_name}={COOKIE_TOKEN}");

            // Open the real raw Feature store before adoption, retaining a handle
            // to verify that cutover fences already-open legacy writers too.
            let legacy = open_legacy_subjektiv_store(&api).unwrap();
            let subject = legacy
                .create_subject(crate::subjektiv::SubjectRole::new("adoption-http").unwrap())
                .unwrap()
                .id;
            let legacy_update = SubjectPermissionsUpdate {
                operation_id: "adoption-http-existing-policy".into(),
                expected_change_id: legacy
                    .subject_permissions(&subject)
                    .unwrap()
                    .map(|policy| policy.change_id),
                execution: vec![workspace_permission("ticket.query")],
                delegation: vec![workspace_permission("ticket.create")],
            };
            let legacy_policy = legacy
                .set_subject_permissions(&subject, &legacy_update)
                .unwrap();

            let runtime_identity =
                RuntimeIdentityMaterial::generate("adoption-http-runtime").unwrap();
            configure_runtime_request_auth(&mut api, &runtime_identity, "adoption-http-runtime");
            let catalog = WorkspaceServerApi::new(api.config.clone(), api.store.clone());
            // Reuse the fixture API, not a second Runtime with the same store lease.
            // Catalog ownership and credential resolution remain production code.
            catalog
                .apis
                .lock()
                .await
                .insert(TEST_WORKSPACE_ID.into(), api.clone());
            catalog
                .routers
                .lock()
                .await
                .insert(TEST_WORKSPACE_ID.into(), build_inner_router(api.clone()));
            let app = workspace_server_router(catalog);
            Self {
                api,
                app,
                owner,
                foreign,
                owner_account,
                cookie,
                origin,
                subject,
                legacy,
                legacy_update,
                legacy_policy,
                runtime_identity,
                _dir: dir,
            }
        }

        fn path(suffix: &str) -> String {
            format!("/api/w/{TEST_WORKSPACE_ID}/config/adoption{suffix}")
        }

        fn bearer(
            &self,
            token: &str,
            method: &str,
            suffix: &str,
            body: Option<Value>,
        ) -> Request<Body> {
            Request::builder()
                .method(method)
                .uri(Self::path(suffix))
                .header("authorization", format!("Bearer {token}"))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    body.map(|body| body.to_string()).unwrap_or_default(),
                ))
                .unwrap()
        }

        fn browser(
            &self,
            method: &str,
            suffix: &str,
            body: Option<Value>,
            origin: Option<&str>,
        ) -> Request<Body> {
            let mut request = Request::builder()
                .method(method)
                .uri(Self::path(suffix))
                .header(axum::http::header::COOKIE, &self.cookie)
                .header(CONTENT_TYPE, "application/json");
            if let Some(origin) = origin {
                request = request.header(ORIGIN, origin);
            }
            request
                .body(Body::from(
                    body.map(|body| body.to_string()).unwrap_or_default(),
                ))
                .unwrap()
        }

        async fn call(&self, request: Request<Body>) -> (StatusCode, Value) {
            tokio::time::timeout(std::time::Duration::from_secs(15), async {
                let response = self.app.clone().oneshot(request).await.unwrap();
                let status = response.status();
                let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
                    .await
                    .unwrap();
                let body = serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                    panic!(
                        "expected JSON response ({status}): {}",
                        String::from_utf8_lossy(&bytes)
                    )
                });
                (status, body)
            })
            .await
            .expect("bounded adoption HTTP request")
        }

        async fn owner_call(
            &self,
            suffix: &str,
            request: &Adoption,
            expected: StatusCode,
        ) -> Value {
            let (status, body) = self
                .call(self.bearer(
                    &self.owner,
                    "POST",
                    suffix,
                    Some(serde_json::to_value(request).unwrap()),
                ))
                .await;
            assert_eq!(status, expected, "{suffix}: {body}");
            body
        }

        async fn source(&self) -> Source {
            let (status, body) = self.call(self.bearer(&self.owner, "GET", "", None)).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            serde_json::from_value(body).unwrap()
        }

        async fn commit(&self, request: &Adoption) -> Receipt {
            serde_json::from_value(self.owner_call("/commit", request, StatusCode::OK).await)
                .unwrap()
        }

        async fn adopt_draft(&self, id: &str) -> Receipt {
            let draft = self.source().await;
            self.commit(&request(&draft, id, vec![])).await
        }

        // Re-read durable authorities, not cached HTTP responses. The raw export
        // API is guarded by FeatureStorage and checks its physical identity.
        fn durable(&self) -> Value {
            let current = self
                .api
                .config_store
                .configuration_current(TEST_WORKSPACE_ID, &self.owner_account)
                .unwrap();
            let identity = self.legacy.database_identity().unwrap();
            let receipts: Vec<(String, String, String)> = self.api.config_store.with_conn(|conn| {
                let mut query = conn.prepare(
                    "SELECT operation_id,request_fingerprint,result_json FROM workspace_configuration_receipts WHERE workspace_id=?1 ORDER BY operation_id",
                )?;
                let rows = query.query_map([TEST_WORKSPACE_ID], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?;
                Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
            }).unwrap();
            json!({
                "state": current.state,
                "history_head": current.history_head,
                "canonical": current.canonical,
                "policies": current.policies,
                "receipts": receipts,
                "legacy_export": self.legacy.preview_permission_transfer(&identity).unwrap(),
                "legacy_receipt": self.legacy.permission_transfer_receipt(&identity).unwrap(),
            })
        }

        fn assert_legacy_writable(&self) {
            // Exact replay is accepted by an unsealed writer, but rejected after
            // cutover even on an already-open handle. It does not create a change.
            assert_eq!(
                self.legacy
                    .set_subject_permissions(&self.subject, &self.legacy_update)
                    .unwrap(),
                self.legacy_policy
            );
        }

        fn assert_legacy_sealed(&self) {
            assert!(matches!(
                self.legacy
                    .set_subject_permissions(&self.subject, &self.legacy_update),
                Err(crate::subjektiv::SubjektivError::PermissionAuthoritySealed),
            ));
        }
    }

    fn core_request(request: &Adoption) -> crate::config_adoption::ConfigurationAdoptionRequest {
        crate::config_adoption::ConfigurationAdoptionRequest {
            operation_id: request.operation_id.clone(),
            expected_head: request.expected_head.clone(),
            expected_catalog_digest: request.expected_catalog_digest.clone(),
            expected_legacy_export: request.expected_legacy_export.clone(),
            source: config_commit_request_from_api(request.source.clone()).unwrap(),
        }
    }

    fn assignment(operation: &str) -> feature_permissions::PermissionAssignment {
        feature_permissions::PermissionAssignment {
            grants: vec![feature_permissions::PermissionReference::Namespace(
                feature_permissions::NamespacePermissions {
                    namespace: feature_permissions::NamespaceId::parse("ticket").unwrap(),
                    keys: vec![feature_permissions::PermissionKey::parse(operation).unwrap()],
                },
            )],
            exclusions: vec![],
            revoked: false,
        }
    }

    fn workspace_permission(operation: &str) -> ScopedPermission {
        ScopedPermission {
            operation: operation.into(),
            resource: PermissionResource::Workspace {
                workspace_id: TEST_WORKSPACE_ID.into(),
            },
        }
    }

    fn request(source: &Source, id: &str, changes: Vec<Change>) -> Adoption {
        Adoption {
            operation_id: id.into(),
            expected_head: source.history_head.clone(),
            expected_catalog_digest: source.catalog_digest.clone(),
            expected_legacy_export: source.expected_legacy_export.clone(),
            source: SourceRequest {
                base_digest: source.state.snapshot.digest.clone(),
                changes,
                entrypoints: source.state.contract.entrypoints.clone(),
            },
        }
    }

    fn update(source: &Source, path: &str, content: &str) -> Change {
        Change::Update {
            path: path.into(),
            expected_digest: source.state.snapshot.entries[path].content_digest.clone(),
            content: content.into(),
        }
    }

    fn create_marker(content: &str) -> Change {
        Change::Create {
            path: MARKER.into(),
            content_type: server_api::ConfigContentType::Text,
            content: content.into(),
        }
    }

    fn permission_path(source: &Source) -> String {
        source
            .state
            .snapshot
            .entries
            .keys()
            .find(|path| path.starts_with("permissions/migrated") && path.ends_with(".dcdl"))
            .expect("migration generated editable permission source")
            .clone()
    }

    fn permission_source(subject: &str, execution: &str, delegation: &str) -> String {
        format!(
            r#"{{
    subjects = [{{
        subject_id = {subject:?};
        execution = {{ namespaces = [{{ namespace = "ticket"; keys = [{execution:?}]; }}]; }};
        delegation = {{ namespaces = [{{ namespace = "ticket"; keys = [{delegation:?}]; }}]; }};
    }}];
    remove_subjects = [];
}}
"#
        )
    }

    #[tokio::test]
    async fn owner_http_repository_validation_is_read_only_and_precedes_seal() {
        let f = Fixture::new().await;
        let source = server_api::RepositorySource {
            kind: server_api::RepositorySourceKind::Ssh,
            uri: "ssh://git@repository.example.test:2222/team/repo.git".into(),
        };
        f.api
            .store
            .upsert_repository(&crate::store::RepositoryRecord {
                workspace_id: TEST_WORKSPACE_ID.into(),
                repository_id: "ssh-repository".into(),
                repository_key: "remote".into(),
                kind: "git".into(),
                provider: None,
                source_fingerprint: crate::repository_source::repository_source_fingerprint(
                    &source,
                ),
                source,
                default_ref: None,
                observed_status: server_api::RepositoryObservedStatus::Unverified,
                observed_at: None,
                created_at: "1".into(),
                updated_at: "1".into(),
            })
            .unwrap();
        let mut non_ssh = f
            .api
            .store
            .get_repository_by_key(TEST_WORKSPACE_ID, "remote")
            .unwrap()
            .unwrap();
        non_ssh.repository_id = "https-repository".into();
        non_ssh.repository_key = "nonssh".into();
        non_ssh.source = server_api::RepositorySource {
            kind: server_api::RepositorySourceKind::Https,
            uri: "https://repository.example.test/team/repo.git".into(),
        };
        non_ssh.source_fingerprint =
            crate::repository_source::repository_source_fingerprint(&non_ssh.source);
        f.api.store.upsert_repository(&non_ssh).unwrap();
        for id in ["deploy", "revoked"] {
            f.api
                .repository_secrets
                .generate_credential(
                    TEST_WORKSPACE_ID,
                    GenerateRepositorySshCredentialRequest {
                        operation_id: format!("generate-{id}"),
                        credential_id: id.into(),
                        name: id.into(),
                    },
                    &f.owner_account,
                )
                .unwrap();
        }
        f.api.config_store.with_conn(|conn| {
            conn.execute("UPDATE repository_ssh_credentials SET status='revoked' WHERE workspace_id=?1 AND credential_id='revoked'", [TEST_WORKSPACE_ID])?;
            Ok(())
        }).unwrap();
        let public_key =
            ssh_key::PrivateKey::from(ssh_key::private::Ed25519Keypair::from_seed(&[37; 32]))
                .public_key()
                .to_openssh()
                .unwrap();
        for (id, hostname, port) in [
            ("matching", "repository.example.test", 2222),
            ("wrong-host", "other.example.test", 2222),
            ("wrong-port", "repository.example.test", 22),
        ] {
            f.api
                .repository_secrets
                .put_host_trust(
                    TEST_WORKSPACE_ID,
                    PutRepositorySshHostTrustRequest {
                        operation_id: format!("trust-{id}"),
                        host_trust_id: id.into(),
                        hostname: hostname.into(),
                        port,
                        host_key: public_key.clone(),
                        expected_fingerprint: None,
                    },
                    &f.owner_account,
                )
                .unwrap();
        }
        let secret_counts = || {
            f.api
                .config_store
                .with_conn(|conn| {
                    [
                        "repository_ssh_credentials",
                        "repository_ssh_credential_keys",
                        "repository_ssh_host_trusts",
                        "repository_ssh_host_trust_keys",
                        "repository_secret_operations",
                        "repository_secret_audit_events",
                        "server_secret_objects",
                    ]
                    .into_iter()
                    .map(|table| {
                        conn.query_row(
                            &format!("SELECT COUNT(*) FROM {table} WHERE workspace_id=?1"),
                            [TEST_WORKSPACE_ID],
                            |r| r.get::<_, i64>(0),
                        )
                        .map_err(Error::from)
                    })
                    .collect::<Result<Vec<_>>>()
                })
                .unwrap()
        };
        let secrets_before = secret_counts();
        let content = |repo: &str, credential: &str, trust: &str, access: &str| {
            format!(
                "{{ repository_access = {{ {repo} = {{ ssh = {{ credential = {credential:?}; host_trust = {trust:?}; access = {access:?}; }}; }}; }}; }}"
            )
        };
        for canonical in [false, true] {
            let current = f.source().await;
            assert_eq!(current.source_stored, canonical);
            let before = f.durable();
            for (case, repo, credential, trust, access, diagnostic) in [
                (
                    "mode",
                    "remote",
                    "deploy",
                    "matching",
                    "not-a-mode",
                    "invalid Repository access config",
                ),
                (
                    "repository",
                    "absent",
                    "deploy",
                    "matching",
                    "read_only",
                    "unknown Repository",
                ),
                (
                    "non-ssh",
                    "nonssh",
                    "deploy",
                    "matching",
                    "read_only",
                    "not an ssh",
                ),
                (
                    "credential",
                    "remote",
                    "absent",
                    "matching",
                    "read_only",
                    "unknown Repository SSH credential",
                ),
                (
                    "inactive",
                    "remote",
                    "revoked",
                    "matching",
                    "read_only",
                    "is not active",
                ),
                (
                    "trust",
                    "remote",
                    "deploy",
                    "absent",
                    "read_only",
                    "unknown Repository SSH host trust",
                ),
                (
                    "host",
                    "remote",
                    "deploy",
                    "wrong-host",
                    "read_only",
                    "does not match host trust",
                ),
                (
                    "port",
                    "remote",
                    "deploy",
                    "wrong-port",
                    "read_only",
                    "does not match host trust",
                ),
            ] {
                let adoption = request(
                    &current,
                    &format!("repository-{canonical}-{case}"),
                    vec![update(
                        &current,
                        "main.dcdl",
                        &content(repo, credential, trust, access),
                    )],
                );
                for endpoint in ["/preview", "/commit"] {
                    let rejected = f
                        .owner_call(endpoint, &adoption, StatusCode::BAD_REQUEST)
                        .await;
                    assert!(
                        rejected.to_string().contains(diagnostic),
                        "{case} {endpoint}: {rejected}"
                    );
                    assert_eq!(
                        f.durable(),
                        before,
                        "{case} {endpoint}: all old authorities must remain unchanged"
                    );
                    assert_eq!(
                        secret_counts(),
                        secrets_before,
                        "validation must not generate keys or secret receipts"
                    );
                    if canonical {
                        f.assert_legacy_sealed();
                    } else {
                        f.assert_legacy_writable();
                    }
                }
            }
            let valid = request(
                &current,
                &format!("valid-repository-{canonical}"),
                vec![update(
                    &current,
                    "main.dcdl",
                    &content(
                        "remote",
                        "deploy",
                        "matching",
                        if canonical { "read_write" } else { "read_only" },
                    ),
                )],
            );
            let preview = f.owner_call("/preview", &valid, StatusCode::OK).await;
            assert_eq!(
                f.durable(),
                before,
                "valid preview must remain a read, before and after cutover"
            );
            assert_eq!(secret_counts(), secrets_before);
            let saved = f.commit(&valid).await;
            assert_eq!(
                serde_json::to_value(&saved.state).unwrap(),
                preview["state"]
            );
            assert_eq!(f.source().await.state, saved.state);
            f.assert_legacy_sealed();
            assert_eq!(
                secret_counts(),
                secrets_before,
                "adoption validates metadata without secret side effects"
            );
        }
        assert!(
            f.api
                .repository_secrets
                .get_credential(TEST_WORKSPACE_ID, "workspace-default", &[])
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn owner_http_edits_migration_draft_and_adopts_source_policy_head_and_receipt() {
        let fixture = Fixture::new().await;
        let before = fixture.durable();
        let draft = fixture.source().await;
        assert!(!draft.source_stored);
        assert_eq!(draft.history_head, None);
        assert!(draft.expected_legacy_export.is_some());
        assert_ne!(
            draft.state.snapshot.digest,
            before["state"]["snapshot"]["digest"]
        );
        assert_eq!(draft.catalog.workspace.as_str(), TEST_WORKSPACE_ID);
        assert!(draft.catalog.namespaces.iter().any(|namespace| {
            namespace
                .declaration
                .permissions
                .iter()
                .any(|permission| permission.key.as_str() == "ticket.query")
        }));
        assert_eq!(draft.policies.len(), 1);
        assert_eq!(draft.policies[0].change_id, fixture.legacy_policy.change_id);
        assert_eq!(
            fixture.durable(),
            before,
            "GET is a read, not a save or seal"
        );
        fixture.assert_legacy_writable();

        let path = permission_path(&draft);
        let edited = format!(
            "{}\n# Owner edited the migration draft before first adoption.\n",
            draft.state.snapshot.entries[&path].content
        );
        let adoption = request(
            &draft,
            "edited-migration-draft",
            vec![update(&draft, &path, &edited)],
        );
        let preview: Preview = serde_json::from_value(
            fixture
                .owner_call("/preview", &adoption, StatusCode::OK)
                .await,
        )
        .unwrap();
        assert_eq!(preview.expected_head, draft.history_head);
        assert!(preview.changed_paths.contains(&path));
        assert!(
            preview.state.snapshot.entries[&path]
                .content
                .contains("# Owner edited the migration draft before first adoption.")
        );
        assert_eq!(
            fixture.durable(),
            before,
            "preview must not seal or persist the draft"
        );
        fixture.assert_legacy_writable();

        // Exercise the real Browser session and configured Origin on commit.
        let (status, wire) = fixture
            .call(fixture.browser(
                "POST",
                "/commit",
                Some(serde_json::to_value(&adoption).unwrap()),
                Some(&fixture.origin),
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{wire}");
        assert_eq!(wire["runtime_application"], "next_current_policy_check");
        let receipt: Receipt = serde_json::from_value(wire).unwrap();
        assert_eq!(
            receipt.runtime_application,
            RuntimePolicyApplication::NextCurrentPolicyCheck
        );
        assert!(receipt.source_stored && receipt.policy_committed);
        assert_eq!(receipt.workspace_id, TEST_WORKSPACE_ID);
        assert_eq!(receipt.operation_id, adoption.operation_id);
        assert_eq!(receipt.state, preview.state);
        let stored = fixture.source().await;
        assert!(stored.source_stored);
        assert_eq!(stored.expected_legacy_export, None);
        assert_eq!(stored.state, receipt.state);
        assert_eq!(
            stored.history_head.as_deref(),
            Some(receipt.history_head.as_str())
        );
        assert_eq!(
            stored.policies, draft.policies,
            "source-only edit preserves policy and change IDs"
        );
        let current = fixture
            .api
            .config_store
            .configuration_current(TEST_WORKSPACE_ID, &fixture.owner_account)
            .unwrap();
        assert!(current.canonical);
        assert_eq!(workspace_config_state_to_api(current.state), receipt.state);
        assert_eq!(
            open_subjektiv_store(&fixture.api)
                .unwrap()
                .subject_permissions(&fixture.subject)
                .unwrap(),
            Some(fixture.legacy_policy.clone())
        );
        let identity = fixture.legacy.database_identity().unwrap();
        let transfer = fixture
            .legacy
            .permission_transfer_receipt(&identity)
            .unwrap()
            .unwrap();
        assert_eq!(
            Some(transfer.snapshot.export_digest),
            draft.expected_legacy_export
        );
        assert_eq!(transfer.snapshot.identity, identity);
        let saved = fixture
            .api
            .config_store
            .configuration_request_result(
                TEST_WORKSPACE_ID,
                &fixture.owner_account,
                &core_request(&adoption),
            )
            .unwrap()
            .unwrap();
        assert_eq!(saved.history_head, receipt.history_head);
        assert_eq!(workspace_config_state_to_api(saved.state), receipt.state);
        assert_eq!(
            saved.runtime_application,
            crate::config_adoption::RuntimePolicyApplication::NextCurrentPolicyCheck
        );
        assert_eq!(
            transfer.transfer_id,
            crate::config_adoption::request_fingerprint(
                TEST_WORKSPACE_ID,
                &fixture.owner_account,
                &core_request(&adoption),
            )
            .unwrap()
        );
        fixture.assert_legacy_sealed();

        // Then edit actual builtin operation assignments through the same routes.
        let changed = permission_source(&fixture.subject, "ticket.create", "ticket.query");
        let policy_edit = request(
            &stored,
            "owner-policy-edit",
            vec![update(&stored, &path, &changed)],
        );
        let preview: Preview = serde_json::from_value(
            fixture
                .owner_call("/preview", &policy_edit, StatusCode::OK)
                .await,
        )
        .unwrap();
        assert_eq!(preview.changed_subjects, vec![fixture.subject.clone()]);
        let next = fixture.commit(&policy_edit).await;
        assert_eq!(next.state, preview.state);
        assert_ne!(next.history_head, receipt.history_head);
        let stored = fixture.source().await;
        assert_eq!(stored.state, next.state);
        let policy = stored
            .policies
            .iter()
            .find(|policy| policy.subject_id == fixture.subject)
            .unwrap();
        assert_eq!(policy.execution, assignment("ticket.create"));
        assert_eq!(policy.delegation, assignment("ticket.query"));
        assert_ne!(policy.change_id, fixture.legacy_policy.change_id);
        let adopted = fixture
            .api
            .config_store
            .adopted_subject_policy(TEST_WORKSPACE_ID, &fixture.subject)
            .unwrap()
            .unwrap();
        assert_eq!(adopted.source.execution.assignment(), policy.execution);
        assert_eq!(adopted.source.delegation.assignment(), policy.delegation);
        assert_eq!(adopted.change_id, policy.change_id);
        // New payload-free keys cannot fabricate broader legacy resource scopes.
        assert_eq!(
            policy.legacy_constraints,
            Some(fixture.legacy_policy.clone())
        );
        fixture.assert_legacy_sealed();
    }

    #[tokio::test]
    async fn competing_http_requests_reject_stale_head_even_after_content_returns_to_a() {
        let fixture = Fixture::new().await;
        fixture.adopt_draft("aba-migrate").await;
        let initial = fixture.source().await;
        fixture
            .commit(&request(&initial, "aba-a", vec![create_marker("A")]))
            .await;
        let a = fixture.source().await;
        // Two independent requests are built from the same observed head.
        let stale = request(&a, "aba-losing-editor", vec![update(&a, MARKER, "loser")]);
        let b = request(&a, "aba-b", vec![update(&a, MARKER, "B")]);
        fixture.commit(&b).await;
        let before = fixture.durable();
        fixture
            .owner_call("/commit", &stale, StatusCode::CONFLICT)
            .await;
        assert_eq!(fixture.durable(), before);
        let b = fixture.source().await;
        fixture
            .commit(&request(&b, "aba-return-a", vec![update(&b, MARKER, "A")]))
            .await;
        let returned = fixture.source().await;
        assert_eq!(
            returned.state.snapshot, a.state.snapshot,
            "the content really returned to A"
        );
        assert_ne!(
            returned.history_head, a.history_head,
            "editing history did not return to A"
        );
        let before = fixture.durable();
        fixture
            .owner_call("/preview", &stale, StatusCode::CONFLICT)
            .await;
        fixture
            .owner_call("/commit", &stale, StatusCode::CONFLICT)
            .await;
        assert_eq!(
            fixture.durable(),
            before,
            "ABA rejection leaves source, policy and receipts intact"
        );
        let reconciled: Reconciliation = serde_json::from_value(
            fixture
                .owner_call("/reconcile", &stale, StatusCode::OK)
                .await,
        )
        .unwrap();
        assert_eq!(reconciled, Reconciliation::NotObserved);
        assert_eq!(fixture.durable(), before);
        fixture.assert_legacy_sealed();
    }

    #[tokio::test]
    async fn full_request_reconcile_recovers_received_commit_result_without_reapplying_old_policy()
    {
        let fixture = Fixture::new().await;
        let draft = fixture.source().await;
        let adoption = request(&draft, "response-lost-after-commit", vec![]);
        let before = fixture.durable();
        let missing: Reconciliation = serde_json::from_value(
            fixture
                .owner_call("/reconcile", &adoption, StatusCode::OK)
                .await,
        )
        .unwrap();
        assert_eq!(missing, Reconciliation::NotObserved);
        assert_eq!(fixture.durable(), before);
        fixture.assert_legacy_writable();

        // Receive a committed HTTP result, then model the consumer losing it.
        // Reconciliation uses the retained original request, not result-derived input.
        let received_result = fixture
            .owner_call("/commit", &adoption, StatusCode::OK)
            .await;
        let after_commit = fixture.durable();
        let reconciled: Reconciliation = serde_json::from_value(
            fixture
                .owner_call("/reconcile", &adoption, StatusCode::OK)
                .await,
        )
        .unwrap();
        let expected: Receipt = serde_json::from_value(received_result).unwrap();
        assert_eq!(
            reconciled,
            Reconciliation::Adopted {
                receipt: expected.clone()
            }
        );
        assert_eq!(fixture.commit(&adoption).await, expected);
        assert_eq!(
            fixture.durable(),
            after_commit,
            "reconcile and exact retry are not new adoptions"
        );

        let current = fixture.source().await;
        let path = permission_path(&current);
        // Explicit removal makes old-receipt resurrection observable.
        let removal = format!(
            "{{ subjects = []; remove_subjects = [{:?}]; }}\n",
            fixture.subject
        );
        fixture
            .commit(&request(
                &current,
                "later-policy-removal",
                vec![update(&current, &path, &removal)],
            ))
            .await;
        assert!(
            fixture
                .api
                .config_store
                .adopted_subject_policy(TEST_WORKSPACE_ID, &fixture.subject)
                .unwrap()
                .is_none()
        );
        let latest = fixture.durable();
        let reconciled: Reconciliation = serde_json::from_value(
            fixture
                .owner_call("/reconcile", &adoption, StatusCode::OK)
                .await,
        )
        .unwrap();
        assert_eq!(
            reconciled,
            Reconciliation::Adopted {
                receipt: expected.clone()
            }
        );
        assert_eq!(fixture.commit(&adoption).await, expected);
        assert_eq!(
            fixture.durable(),
            latest,
            "historical receipt must not resurrect removed policy or head"
        );
        assert!(
            open_subjektiv_store(&fixture.api)
                .unwrap()
                .subject_permissions(&fixture.subject)
                .unwrap()
                .is_none()
        );
        fixture.assert_legacy_sealed();
    }

    #[tokio::test]
    async fn reconcile_rejects_same_operation_id_with_any_changed_request_fingerprint() {
        let fixture = Fixture::new().await;
        let draft = fixture.source().await;
        let adoption = request(&draft, "fingerprint-migration", vec![]);
        let receipt = fixture.commit(&adoption).await;
        let before = fixture.durable();
        let mut variants = Vec::new();
        let mut changed = adoption.clone();
        changed.expected_head = Some(receipt.history_head.clone());
        variants.push(("head", changed));
        let mut changed = adoption.clone();
        changed.expected_catalog_digest.push_str("-different");
        variants.push(("catalog", changed));
        let mut changed = adoption.clone();
        changed.expected_legacy_export = None;
        variants.push(("legacy export", changed));
        let mut changed = adoption.clone();
        changed.source.base_digest.push_str("-different");
        variants.push(("base content", changed));
        let mut changed = adoption.clone();
        changed
            .source
            .changes
            .push(create_marker("not the same request"));
        variants.push(("changes", changed));
        let mut changed = adoption.clone();
        changed.source.entrypoints.push("other.dcdl".into());
        variants.push(("entrypoints", changed));
        for (field, changed) in variants {
            fixture
                .owner_call("/reconcile", &changed, StatusCode::CONFLICT)
                .await;
            fixture
                .owner_call("/commit", &changed, StatusCode::CONFLICT)
                .await;
            assert_eq!(
                fixture.durable(),
                before,
                "changed {field} must not adopt or modify a receipt"
            );
        }
        let exact: Reconciliation = serde_json::from_value(
            fixture
                .owner_call("/reconcile", &adoption, StatusCode::OK)
                .await,
        )
        .unwrap();
        assert_eq!(exact, Reconciliation::Adopted { receipt });
        assert_eq!(fixture.durable(), before);
    }

    #[tokio::test]
    async fn http_owner_and_cookie_origin_denials_leave_all_authorities_and_legacy_writer_intact() {
        let fixture = Fixture::new().await;
        for canonical in [false, true] {
            let source = fixture.source().await;
            assert_eq!(source.source_stored, canonical);
            let change = if canonical {
                update(&source, MARKER, "second authorized value")
            } else {
                create_marker("first authorized value")
            };
            let adoption = request(
                &source,
                &format!("denied-cutover-{canonical}"),
                vec![change],
            );
            let wire = serde_json::to_value(&adoption).unwrap();
            let before = fixture.durable();
            for (method, suffix) in [
                ("GET", ""),
                ("POST", "/preview"),
                ("POST", "/commit"),
                ("POST", "/reconcile"),
            ] {
                let body = (method == "POST").then(|| wire.clone());
                let anonymous = Request::builder()
                    .method(method)
                    .uri(Fixture::path(suffix))
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        body.as_ref().map(Value::to_string).unwrap_or_default(),
                    ))
                    .unwrap();
                let runtime = runtime_source_request(
                    &fixture.runtime_identity,
                    None,
                    method,
                    &Fixture::path(suffix),
                    body.as_ref()
                        .map(|value| serde_json::to_vec(value).unwrap())
                        .unwrap_or_default(),
                );
                let mut runtime_with_owner = runtime_source_request(
                    &fixture.runtime_identity,
                    None,
                    method,
                    &Fixture::path(suffix),
                    body.as_ref()
                        .map(|value| serde_json::to_vec(value).unwrap())
                        .unwrap_or_default(),
                );
                runtime_with_owner.headers_mut().insert(
                    axum::http::header::AUTHORIZATION,
                    format!("Bearer {}", fixture.owner).parse().unwrap(),
                );
                for (principal, request) in [
                    (
                        "nonowner",
                        fixture.bearer(&fixture.foreign, method, suffix, body.clone()),
                    ),
                    ("unauthenticated", anonymous),
                    ("runtime proof", runtime),
                    // A valid owner credential must not hide the Runtime source.
                    ("runtime proof plus owner bearer", runtime_with_owner),
                ] {
                    let (status, response) = fixture.call(request).await;
                    assert_eq!(
                        status,
                        StatusCode::FORBIDDEN,
                        "{principal} {method} {suffix}: {response}"
                    );
                    assert_eq!(fixture.durable(), before, "{principal} {suffix}");
                    if canonical {
                        fixture.assert_legacy_sealed();
                    } else {
                        fixture.assert_legacy_writable();
                    }
                }
                if method == "POST" {
                    for origin in [None, Some("https://attacker.invalid")] {
                        let (status, response) = fixture
                            .call(fixture.browser(method, suffix, body.clone(), origin))
                            .await;
                        assert_eq!(
                            status,
                            StatusCode::FORBIDDEN,
                            "cookie {origin:?} {suffix}: {response}"
                        );
                        assert_eq!(fixture.durable(), before);
                        if canonical {
                            fixture.assert_legacy_sealed();
                        } else {
                            fixture.assert_legacy_writable();
                        }
                    }
                }
            }
            // A Browser GET does not require a mutation Origin, and the same
            // valid adoption remains possible after every denied request.
            let (status, response) = fixture.call(fixture.browser("GET", "", None, None)).await;
            assert_eq!(status, StatusCode::OK, "{response}");
            assert_eq!(serde_json::from_value::<Source>(response).unwrap(), source);
            fixture.commit(&adoption).await;
            fixture.assert_legacy_sealed();
        }
    }

    #[tokio::test]
    async fn invalid_http_source_and_old_source_tree_writer_cannot_partially_adopt() {
        let fixture = Fixture::new().await;
        let draft = fixture.source().await;
        let path = permission_path(&draft);
        let before = fixture.durable();
        let invalid = [
            ("syntax", "{ subjects = [".to_owned()),
            (
                "unknown-key",
                permission_source(
                    &fixture.subject,
                    "ticket.not_a_builtin_operation",
                    "ticket.query",
                ),
            ),
            (
                "missing-subject",
                permission_source(
                    "nonexistent-adoption-subject",
                    "ticket.create",
                    "ticket.query",
                ),
            ),
            (
                "unknown-set",
                format!(
                    "{{ subjects = [{{ subject_id = {:?}; execution = {{ sets = [\"unknown-adoption-set\"]; }}; delegation = {{}}; }}]; remove_subjects = []; }}",
                    fixture.subject
                ),
            ),
        ];
        for (name, content) in invalid {
            let adoption = request(
                &draft,
                &format!("invalid-{name}"),
                vec![update(&draft, &path, &content)],
            );
            for suffix in ["/preview", "/commit"] {
                let (status, response) = fixture
                    .call(fixture.bearer(
                        &fixture.owner,
                        "POST",
                        suffix,
                        Some(serde_json::to_value(&adoption).unwrap()),
                    ))
                    .await;
                assert!(
                    status.is_client_error(),
                    "{name} {suffix}: {status} {response}"
                );
                assert_eq!(
                    fixture.durable(),
                    before,
                    "invalid {name} leaves no source, policy, head, receipt or seal"
                );
                fixture.assert_legacy_writable();
            }
        }
        for canonical in [false, true] {
            if canonical {
                fixture.adopt_draft("old-writer-cutover").await;
            }
            let current = fixture.source().await;
            let old = SourceRequest {
                base_digest: current.state.snapshot.digest,
                changes: vec![create_marker("old writer must not persist")],
                entrypoints: current.state.contract.entrypoints,
            };
            let before = fixture.durable();
            let response = Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/w/{TEST_WORKSPACE_ID}/config/source-tree/commit"
                ))
                .header("authorization", format!("Bearer {}", fixture.owner))
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&old).unwrap()))
                .unwrap();
            let (status, response) = fixture.call(response).await;
            assert_eq!(
                status,
                StatusCode::CONFLICT,
                "old writer canonical={canonical}: {response}"
            );
            assert_eq!(fixture.durable(), before);
            if canonical {
                fixture.assert_legacy_sealed();
            } else {
                fixture.assert_legacy_writable();
            }
        }
    }
}
