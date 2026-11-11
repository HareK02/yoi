// Included inside server::tests to reuse its catalog, identity and storage fixtures.
fn adopt_test_config_as_owner(
    api: &WorkspaceApi,
    source: &crate::config_source::ConfigCommitRequest,
) -> crate::Result<crate::config_source::WorkspaceConfigState> {
    let owner = test_owner_actor();
    api.refresh_plugin_permission_registry()?;
    let host = api.permission_registry.read().unwrap().host().clone();
    let current = api
        .config_store
        .configuration_current(api.workspace_id(), &owner.account_id)?;
    let request = crate::config_adoption::ConfigurationAdoptionRequest {
        operation_id: Uuid::now_v7().to_string(),
        expected_head: current.history_head,
        expected_catalog_digest: crate::config_adoption::catalog_digest(&host)?,
        source: source.clone(),
    };
    api.config_store
        .adopt_configuration(
            api.workspace_id(),
            &owner.account_id,
            &request,
            api.config_schema_registry.compose()?,
            &host,
        )
        .map(|result| result.state)
}

fn replace_config_plugin_subject_policy_as_owner(
    api: &WorkspaceApi,
    subject: &str,
    execution: feature_permissions::PermissionAssignment,
    delegation: feature_permissions::PermissionAssignment,
) -> server_api::authorization::CurrentSubjectPolicy {
    let native = open_subjektiv_store(api).unwrap();
    let identity = native.database_identity().unwrap();
    native
        .with_permission_authority_guard(&identity, |authority| {
            authority.require_subject(subject)?;
            let registry = api.permission_registry.read().unwrap();
            let request = server_api::subject_policy::SubjectPermissionPolicyMutation {
                operation_id: Uuid::now_v7().to_string(),
                expected_change_id: api
                    .config_store
                    .subject_permission_policy_change_id(api.workspace_id(), subject)
                    .unwrap(),
                action: server_api::subject_policy::SubjectPermissionPolicyAction::Replace {
                    execution,
                    delegation,
                },
            };
            let response = api
                .config_store
                .mutate_subject_permission_policy(
                    api.workspace_id(),
                    &test_owner_actor().account_id,
                    subject,
                    &request,
                    registry.host(),
                )
                .unwrap();
            assert_eq!(
                api.config_store
                    .mutate_subject_permission_policy(
                        api.workspace_id(),
                        &test_owner_actor().account_id,
                        subject,
                        &request,
                        registry.host(),
                    )
                    .unwrap(),
                response,
                "owner policy replay returns its receipt"
            );
            Ok(response.policy.unwrap())
        })
        .unwrap()
}

mod configuration_adoption_http_integration {
    //! Ordinary Server library integration tests: real catalog router (including
    //! cookie-origin middleware), owner/session lookup, builtin permission catalog,
    //! automatic migration, guarded Feature storage and Server SQLite adoption. Only unused
    //! Runtime execution is the existing deterministic fixture. Existing route tests
    //! use Router oneshot; cli_tcp tests connect the production CLI library and
    //! generated client over loopback TCP to this same Server and real SQLite.
    //! The response-loss case fails one outbound body only after a real commit;
    //! it does not simulate arbitrary TCP loss or OS crashes. The separate
    //! configuration_transfer/config_adoption suites own store faults.
    use super::*;
    use server_api::authorization::{
        PermissionResource, ScopedPermission, SubjectPermissionsUpdate,
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
        subject_policy: server_api::authorization::CurrentSubjectPolicy,
        runtime_identity: RuntimeIdentityMaterial,
        _dir: tempfile::TempDir,
    }

    impl Fixture {
        async fn new() -> Self {
            Self::with_publication_host(HostExecutors::default()).await
        }

        async fn with_publication_host(host: HostExecutors) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let mut api = test_api(dir.path()).await;
            api.host_executors = host;
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

            // Startup has already transferred native authority. Seed the current
            // Subject domain through its owner/CAS operation, not the sealed writer.
            let legacy = open_subjektiv_store(&api).unwrap();
            let subject = open_subjektiv_store(&api)
                .unwrap()
                .create_subject(crate::subjektiv::SubjectRole::new("adoption-http").unwrap())
                .unwrap()
                .id;
            let legacy_update = SubjectPermissionsUpdate {
                operation_id: "adoption-http-rejected-native-write".into(),
                expected_change_id: None,
                execution: vec![workspace_permission("ticket.query")],
                delegation: vec![workspace_permission("ticket.create")],
            };
            let subject_policy = replace_config_plugin_subject_policy_as_owner(
                &api,
                &subject,
                assignment("ticket.query"),
                assignment("ticket.create"),
            );
            let state = api
                .config_store
                .load_workspace_config(TEST_WORKSPACE_ID)
                .unwrap()
                .unwrap();
            let main = config_source::VirtualPath::parse("main.dcdl").unwrap();
            adopt_test_config_as_owner(&api, &crate::config_source::ConfigCommitRequest {
                base_digest: state.snapshot.digest.clone(),
                entrypoints: state.contract.entrypoints.clone(),
                changes: vec![
                    config_source::ConfigTreeChange::Create {
                        path: config_source::VirtualPath::parse("permissions/owner.dcdl").unwrap(),
                        content_type: config_source::ConfigContentType::Decodal,
                        content: permission_source("ticket.query", "ticket.create"),
                    },
                    config_source::ConfigTreeChange::Update {
                        path: main.clone(),
                        expected_digest: state.snapshot.get(&main).unwrap().content_digest.clone(),
                        content: "{ permissions = import \"permissions/owner.dcdl\"; } as WorkspaceConfigSchema\n".into(),
                    },
                ],
            }).unwrap();

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
                subject_policy,
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
                "policies": current.policies,
                "receipts": receipts,
                "subject_policy": self.api.config_store.current_subject_permission_policy(TEST_WORKSPACE_ID, &self.subject).unwrap(),
            })
        }

        fn assert_subject_policy_unchanged(&self) {
            self.assert_native_writer_denied();
            assert_eq!(
                self.api
                    .config_store
                    .current_subject_permission_policy(TEST_WORKSPACE_ID, &self.subject)
                    .unwrap(),
                Some(self.subject_policy.clone()),
                "shared configuration never replaces individual Subject policy",
            );
        }

        fn assert_source_policy_unchanged(&self, source: &Source) {
            self.assert_subject_policy_unchanged();
            assert_eq!(source.policies.len(), 1);
            let policy = &source.policies[0];
            assert_eq!(policy.subject_id, self.subject);
            assert_eq!(policy.change_id, self.subject_policy.change_id);
            assert_eq!(policy.execution, self.subject_policy.execution);
            assert_eq!(policy.delegation, self.subject_policy.delegation);
            assert_eq!(
                policy.legacy_constraints,
                self.subject_policy.legacy_constraints
            );
            let registry = self.api.permission_registry.read().unwrap();
            assert_eq!(
                policy.execution_resolution,
                *registry
                    .host()
                    .resolve(Some(&policy.execution))
                    .projection()
            );
            assert_eq!(
                policy.delegation_resolution,
                *registry
                    .host()
                    .resolve(Some(&policy.delegation))
                    .projection()
            );
        }

        fn assert_native_writer_denied(&self) {
            assert!(matches!(
                self.legacy
                    .set_subject_permissions(&self.subject, &self.legacy_update),
                Err(crate::subjektiv::SubjektivError::AuthorizationDenied(_)),
            ));
        }
    }

    #[tokio::test]
    async fn permission_management_uses_owner_and_atomic_source_catalog_resolution() {
        let f = Fixture::new().await;
        let path = format!("/api/w/{TEST_WORKSPACE_ID}/permission-catalog");
        for (token, expected) in [
            (&f.owner, StatusCode::OK),
            (&f.foreign, StatusCode::FORBIDDEN),
        ] {
            let request = Request::builder()
                .method("GET")
                .uri(&path)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap();
            let (status, _) = f.call(request).await;
            assert_eq!(status, expected);
        }
        let stored = f.adopt_draft("permission-resolution-source").await;
        let source = f.source().await;
        assert!(source.source_stored);
        assert_eq!(
            source.history_head.as_deref(),
            Some(stored.history_head.as_str())
        );
        let registry = f.api.permission_registry.read().unwrap();
        assert_eq!(source.catalog, registry.host().catalog());
        assert_eq!(
            source.catalog_digest,
            crate::config_adoption::catalog_digest(registry.host()).unwrap()
        );
        drop(registry);
        f.assert_source_policy_unchanged(&source);
        let assignment = assignment("ticket.read");
        let body = json!({"assignment": assignment});
        let request = Request::builder()
            .method("POST")
            .uri(format!("/api/w/{TEST_WORKSPACE_ID}/permissions/validate"))
            .header("authorization", format!("Bearer {}", f.owner))
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let (status, resolution) = f.call(request).await;
        assert_eq!(status, StatusCode::OK, "{resolution}");
        assert!(resolution["allowed"].get("ticket.read").is_some());
    }

    struct CliBearer(String);

    impl server_api::client_support::RequestAuthorizer for CliBearer {
        fn authorize(
            &self,
            _: server_api::client_support::AuthorizerRequest<'_>,
        ) -> std::result::Result<
            reqwest::header::HeaderMap,
            server_api::client_support::AuthorizationError,
        > {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.0).parse().unwrap(),
            );
            Ok(headers)
        }
    }

    #[derive(Default)]
    struct CommitResponseLoss {
        requests: Vec<Vec<u8>>,
        lost_receipt: Option<Receipt>,
    }

    async fn fail_first_committed_response(
        axum::extract::State(evidence): axum::extract::State<
            Arc<std::sync::Mutex<CommitResponseLoss>>,
        >,
        request: Request<Body>,
        next: axum::middleware::Next,
    ) -> axum::response::Response {
        if request.method() != axum::http::Method::POST
            || request.uri().path() != Fixture::path("/commit")
        {
            return next.run(request).await;
        }
        let (parts, body) = request.into_parts();
        let bytes = to_bytes(body, 4 * 1024 * 1024).await.unwrap();
        evidence.lock().unwrap().requests.push(bytes.to_vec());
        let response = next
            .run(Request::from_parts(parts, Body::from(bytes)))
            .await;
        if !response.status().is_success() || evidence.lock().unwrap().lost_receipt.is_some() {
            return response;
        }

        // This runs after the actual router has returned its committed receipt.
        // Preserve the request and response headers; fail only the outbound body.
        let (parts, body) = response.into_parts();
        let bytes = to_bytes(body, 4 * 1024 * 1024).await.unwrap();
        let receipt: Receipt = serde_json::from_slice(&bytes).unwrap();
        evidence.lock().unwrap().lost_receipt = Some(receipt);
        let truncated = bytes.slice(..bytes.len() / 2);
        let body = Body::from_stream(futures::stream::iter([
            Ok(truncated),
            Err(std::io::Error::other(
                "injected body failure after Server commit",
            )),
        ]));
        axum::response::Response::from_parts(parts, body)
    }

    struct CliServer {
        origin: String,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for CliServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    impl CliServer {
        async fn start(fixture: &Fixture) -> Self {
            Self::serve(fixture.app.clone()).await
        }

        async fn with_commit_response_loss(
            fixture: &Fixture,
        ) -> (Self, Arc<std::sync::Mutex<CommitResponseLoss>>) {
            let evidence = Arc::new(std::sync::Mutex::new(CommitResponseLoss::default()));
            let app = fixture
                .app
                .clone()
                .layer(axum::middleware::from_fn_with_state(
                    evidence.clone(),
                    fail_first_committed_response,
                ));
            (Self::serve(app).await, evidence)
        }

        async fn serve(app: Router) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let task = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            Self { origin, task }
        }

        fn client(&self, token: &str) -> server_api::ConfigurationAdoptionApiClient<CliBearer> {
            server_api::ConfigurationAdoptionApiClient::builder(&self.origin)
                .unwrap()
                .client(
                    reqwest::Client::builder()
                        .no_proxy()
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                        .unwrap(),
                )
                .authorizer(CliBearer(token.to_owned()))
                .request_timeout(std::time::Duration::from_secs(15))
                .response_body_limit(4 * 1024 * 1024)
                .build()
                .unwrap()
        }

        async fn run(
            &self,
            token: &str,
            directory: &std::path::Path,
            command: yoi::config_cli::Command,
        ) -> std::result::Result<String, yoi::config_cli::Error> {
            let api = self.client(token);
            tokio::time::timeout(
                std::time::Duration::from_secs(45),
                yoi::config_cli::run(
                    &api,
                    &self.origin,
                    TEST_WORKSPACE_ID,
                    &yoi::config_cli::Options {
                        command,
                        directory: directory.to_owned(),
                    },
                ),
            )
            .await
            .expect("bounded CLI command against real Server")
        }
    }

    #[tokio::test]
    async fn cli_tcp_lost_commit_response_recovers_real_receipt_without_reapplying_policy_or_losing_edits()
     {
        use yoi::config_cli::Command::*;
        for recovery in [Reconcile, Retry] {
            let f = Fixture::new().await;
            let (server, evidence) = CliServer::with_commit_response_loss(&f).await;
            let dir = tempfile::tempdir().unwrap();
            let checkout = dir.path().join("checkout");
            server.run(&f.owner, &checkout, Pull).await.unwrap();
            let draft = f.source().await;
            let path = permission_path(&draft);
            let local = checkout.join("source").join(&path);
            let sent_text = permission_source("ticket.create", "ticket.query");
            std::fs::write(&local, &sent_text).unwrap();

            let error = server.run(&f.owner, &checkout, Push).await.unwrap_err();
            assert_eq!(error.exit_code, 7, "{recovery:?}: {error}");
            assert!(error.message.contains("unknown"), "{error}");
            let metadata = checkout.join(".yoi-config/state.json");
            let unknown: Value =
                serde_json::from_slice(&std::fs::read(&metadata).unwrap()).unwrap();
            let pending: Adoption = serde_json::from_value(unknown["pending"].clone()).unwrap();
            assert_eq!(unknown["base"]["history_head"], json!(draft.history_head));
            let (receipt, original_wire) = {
                let captured = evidence.lock().unwrap();
                assert_eq!(
                    captured.requests.len(),
                    1,
                    "unknown push must not automatically resend"
                );
                (
                    captured
                        .lost_receipt
                        .clone()
                        .expect("real Server returned a receipt before body failure"),
                    captured.requests[0].clone(),
                )
            };
            assert_eq!(
                serde_json::from_slice::<Value>(&original_wire).unwrap(),
                serde_json::to_value(&pending).unwrap()
            );
            assert_eq!(receipt.operation_id, pending.operation_id);
            let saved = f
                .api
                .config_store
                .configuration_request_result(
                    TEST_WORKSPACE_ID,
                    &f.owner_account,
                    &core_request(&pending),
                )
                .unwrap()
                .expect("actual SQLite receipt exists despite CLI Unknown");
            assert_eq!(saved.history_head, receipt.history_head);
            assert_eq!(workspace_config_state_to_api(saved.state), receipt.state);
            let committed = f.source().await;
            assert_eq!(committed.state, receipt.state);
            assert_eq!(
                committed.history_head.as_deref(),
                Some(receipt.history_head.as_str())
            );
            assert_eq!(
                shared_permissions(&committed).worker_default.assignment(),
                assignment("ticket.create")
            );
            f.assert_source_policy_unchanged(&committed);
            f.assert_subject_policy_unchanged();
            assert_eq!(f.durable()["receipts"].as_array().unwrap().len(), 2);
            f.assert_native_writer_denied();

            // The editor moves on while the original request is still unresolved.
            let later_text = permission_source("ticket.query", "ticket.create");
            std::fs::write(&local, &later_text).unwrap();
            let marker = checkout.join("source").join(MARKER);
            std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
            std::fs::write(&marker, "local edit after the lost response").unwrap();
            assert_eq!(std::fs::read_to_string(&local).unwrap(), later_text);

            // A real Web owner then clears shared defaults. Historical receipt
            // recovery must not resurrect it, rewind the head, or send later edits.
            let removal = "{ sets = []; worker_default = {}; }\n";
            let web = request(
                &committed,
                "response-loss-later-web-removal",
                vec![update(&committed, &path, &removal)],
            );
            let response = reqwest::Client::builder()
                .no_proxy()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap()
                .post(format!("{}{}", server.origin, Fixture::path("/commit")))
                .header(reqwest::header::COOKIE, &f.cookie)
                .header(reqwest::header::ORIGIN, &f.origin)
                .json(&web)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), reqwest::StatusCode::OK);
            let web_receipt: Receipt = response.json().await.unwrap();
            assert_ne!(web_receipt.history_head, receipt.history_head);
            let cleared = f.source().await;
            f.assert_source_policy_unchanged(&cleared);
            assert_eq!(
                shared_permissions(&cleared).worker_default.assignment(),
                Default::default()
            );
            f.assert_subject_policy_unchanged();
            let current = f.durable();

            let recovered = server.run(&f.owner, &checkout, recovery).await.unwrap();
            assert!(
                recovered.contains(&format!("operation={}", receipt.operation_id)),
                "{recovered}"
            );
            assert!(
                recovered.contains(&format!("head={}", receipt.history_head)),
                "{recovered}"
            );
            assert!(
                recovered.contains("source_stored=true policy_committed=true"),
                "{recovered}"
            );
            assert_eq!(
                f.durable(),
                current,
                "{recovery:?} must only recover the historical receipt"
            );
            assert_eq!(std::fs::read_to_string(&local).unwrap(), later_text);
            assert_eq!(
                std::fs::read_to_string(&marker).unwrap(),
                "local edit after the lost response"
            );
            let resolved: Value =
                serde_json::from_slice(&std::fs::read(&metadata).unwrap()).unwrap();
            assert!(resolved["pending"].is_null());
            assert_eq!(
                resolved["base"]["history_head"],
                json!(receipt.history_head)
            );
            assert_eq!(
                resolved["base"]["state"],
                serde_json::to_value(&receipt.state).unwrap()
            );
            {
                let captured = evidence.lock().unwrap();
                match recovery {
                    Reconcile => {
                        assert_eq!(captured.requests.len(), 2, "reconciliation sends no commit")
                    }
                    Retry => {
                        assert_eq!(captured.requests.len(), 3);
                        assert_eq!(
                            captured.requests[2], original_wire,
                            "exact retry sends the original payload, not post-send edits"
                        );
                    }
                    _ => unreachable!(),
                }
            }
            let diff = server.run(&f.owner, &checkout, Diff).await.unwrap();
            assert!(
                diff.contains(&format!("update {path}"))
                    && diff.contains(&format!("create {MARKER}")),
                "{diff}"
            );
            let error = server.run(&f.owner, &checkout, Push).await.unwrap_err();
            assert_eq!(
                error.exit_code, 4,
                "later local edits need fresh-head conflict resolution: {error}"
            );
            assert_eq!(f.durable(), current);
            assert_eq!(std::fs::read_to_string(&local).unwrap(), later_text);
            assert_eq!(
                std::fs::read_to_string(&marker).unwrap(),
                "local edit after the lost response"
            );
        }
    }

    #[tokio::test]
    async fn cli_tcp_unmodified_push_repulls_identical_shared_source_without_changing_subject_policy()
     {
        use yoi::config_cli::Command::*;
        let f = Fixture::new().await;
        let server = CliServer::start(&f).await;
        let dir = tempfile::tempdir().unwrap();
        let checkout = dir.path().join("checkout");
        let before = f.durable();
        let draft = f.source().await;
        let pulled = server.run(&f.owner, &checkout, Pull).await.unwrap();
        assert!(pulled.contains("source_stored=true"), "{pulled}");
        let diff = server.run(&f.owner, &checkout, Diff).await.unwrap();
        assert!(
            !diff.contains("\ncreate ")
                && !diff.contains("\nupdate ")
                && !diff.contains("\ndelete "),
            "{diff}"
        );
        server.run(&f.owner, &checkout, Validate).await.unwrap();
        server.run(&f.owner, &checkout, Preview).await.unwrap();
        assert_eq!(f.durable(), before, "pull, diff and preview do not adopt");

        let pushed = server.run(&f.owner, &checkout, Push).await.unwrap();
        assert!(
            pushed.contains("source_stored=true policy_committed=true"),
            "{pushed}"
        );
        assert!(pushed.contains("NextCurrentPolicyCheck"), "{pushed}");
        let stored = f.source().await;
        assert!(stored.source_stored);
        assert_eq!(stored.state, draft.state);
        assert_eq!(stored.policies, draft.policies);
        let current = f
            .api
            .config_store
            .configuration_current(TEST_WORKSPACE_ID, &f.owner_account)
            .unwrap();
        assert_eq!(workspace_config_state_to_api(current.state), stored.state);
        assert_eq!(current.history_head, stored.history_head);
        f.assert_native_writer_denied();

        let repull = dir.path().join("repull");
        server.run(&f.owner, &repull, Pull).await.unwrap();
        for (path, entry) in &stored.state.snapshot.entries {
            assert_eq!(
                std::fs::read_to_string(repull.join("source").join(path)).unwrap(),
                entry.content
            );
        }
        let canonical = f.durable();
        server.run(&f.owner, &checkout, Push).await.unwrap();
        let unchanged = f.source().await;
        assert_eq!(unchanged.state, stored.state);
        assert_eq!(unchanged.policies, stored.policies);
        // A new operation may record another history event; it must not change
        // configuration meaning or reseal/rewrite the legacy transfer.
        assert_eq!(f.durable()["legacy_receipt"], canonical["legacy_receipt"]);
    }

    #[tokio::test]
    async fn cli_tcp_shared_permission_text_preserves_sets_exact_keys_and_exclusions() {
        use feature_permissions::{
            PermissionKey, PermissionReference, PermissionSetId, WorkspaceId,
        };
        use yoi::config_cli::Command::*;
        let f = Fixture::new().await;
        let server = CliServer::start(&f).await;
        let dir = tempfile::tempdir().unwrap();
        let checkout = dir.path().join("checkout");
        server.run(&f.owner, &checkout, Pull).await.unwrap();
        let draft = f.source().await;
        let path = permission_path(&draft);
        let text = r#"{
    sets = [{ id = "owner-tools"; sets = ["ticket"];
        namespaces = [{ namespace = "worker"; keys = ["worker.observe"]; }]; }];
    worker_default = {
        sets = ["owner-tools"];
        exclude_namespaces = [{ namespace = "ticket"; keys = ["ticket.create"]; }];
    };
}
"#;
        std::fs::write(checkout.join("source").join(&path), text).unwrap();
        let before = f.durable();
        server.run(&f.owner, &checkout, Validate).await.unwrap();
        let preview = server.run(&f.owner, &checkout, Preview).await.unwrap();
        assert!(
            preview.contains(&path) && preview.contains("changed_subjects=[]"),
            "{preview}"
        );
        assert_eq!(f.durable(), before);
        server.run(&f.owner, &checkout, Push).await.unwrap();
        let source = f.source().await;
        let shared = shared_permissions(&source);
        let assignment = shared.worker_default.assignment();
        assert_eq!(
            assignment.grants,
            vec![PermissionReference::Set(
                PermissionSetId::parse("owner-tools").unwrap()
            )]
        );
        let builtin = worker::feature::permissions::builtin_permission_host(
            WorkspaceId::parse(TEST_WORKSPACE_ID).unwrap(),
        )
        .unwrap();
        assert_eq!(
            source.catalog,
            builtin.catalog(),
            "source cannot replace Feature-owned catalog"
        );
        let host = crate::policy_config::configured_host(&builtin, &shared).unwrap();
        let resolved = host.resolve(Some(&assignment));
        let key = |value| PermissionKey::parse(value).unwrap();
        assert!(
            resolved
                .projection()
                .allowed
                .contains_key(&key("ticket.query"))
        );
        assert!(
            resolved
                .projection()
                .allowed
                .contains_key(&key("worker.observe"))
        );
        assert!(
            !resolved
                .projection()
                .allowed
                .contains_key(&key("ticket.create")),
            "exclusions remain effective"
        );
        assert!(
            !resolved
                .projection()
                .allowed
                .contains_key(&key("worker.observe.list")),
            "dot prefixes are not children"
        );
        f.assert_source_policy_unchanged(&source);
        f.assert_subject_policy_unchanged();
        let repull = dir.path().join("repull");
        server.run(&f.owner, &repull, Pull).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(checkout.join("source").join(&path)).unwrap(),
            text
        );
        assert_eq!(
            std::fs::read_to_string(repull.join("source").join(&path)).unwrap(),
            source.state.snapshot.entries[&path].content
        );
        server.run(&f.owner, &repull, Push).await.unwrap();
        let unchanged = f.source().await;
        assert_eq!(unchanged.state, source.state);
        assert_eq!(unchanged.catalog, source.catalog);
        f.assert_subject_policy_unchanged();
    }

    #[tokio::test]
    async fn cli_tcp_file_add_delete_and_shared_default_removal_preserve_subject_policy() {
        use yoi::config_cli::Command::*;
        let f = Fixture::new().await;
        let server = CliServer::start(&f).await;
        let dir = tempfile::tempdir().unwrap();
        let checkout = dir.path().join("checkout");
        server.run(&f.owner, &checkout, Pull).await.unwrap();
        server.run(&f.owner, &checkout, Push).await.unwrap();
        let initial = f.source().await;
        let policy_path = permission_path(&initial);
        let marker = checkout.join("source").join(MARKER);
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(&marker, "owner added source note").unwrap();
        let diff = server.run(&f.owner, &checkout, Diff).await.unwrap();
        assert!(diff.contains(&format!("create {MARKER}")), "{diff}");
        assert!(
            !diff.contains("update main.dcdl"),
            "unchanged source omitted: {diff}"
        );
        let preview = server.run(&f.owner, &checkout, Preview).await.unwrap();
        assert!(preview.contains(MARKER), "{preview}");
        server.run(&f.owner, &checkout, Push).await.unwrap();
        let added = f.source().await;
        assert_eq!(
            added.state.snapshot.entries[MARKER].content,
            "owner added source note"
        );
        assert_eq!(added.policies, initial.policies);
        assert_eq!(
            added.state.snapshot.entries["main.dcdl"],
            initial.state.snapshot.entries["main.dcdl"]
        );

        std::fs::remove_file(&marker).unwrap();
        std::fs::remove_file(checkout.join("source").join(&policy_path)).unwrap();
        // Remove the import as well as its file. Omitting permissions is not revocation.
        std::fs::write(
            checkout.join("source/main.dcdl"),
            "{} as WorkspaceConfigSchema\n",
        )
        .unwrap();
        let diff = server.run(&f.owner, &checkout, Diff).await.unwrap();
        assert!(
            diff.contains(&format!("delete {MARKER}"))
                && diff.contains(&format!("delete {policy_path}")),
            "{diff}"
        );
        assert!(diff.contains("update main.dcdl"), "{diff}");
        let before = f.durable();
        let preview = server.run(&f.owner, &checkout, Preview).await.unwrap();
        assert!(
            preview.contains(MARKER) && preview.contains(&policy_path),
            "{preview}"
        );
        assert!(preview.contains("changed_subjects=[]"), "{preview}");
        assert_eq!(f.durable(), before);
        server.run(&f.owner, &checkout, Push).await.unwrap();
        let omitted = f.source().await;
        assert!(!omitted.state.snapshot.entries.contains_key(MARKER));
        assert!(!omitted.state.snapshot.entries.contains_key(&policy_path));
        assert_eq!(
            omitted.policies, added.policies,
            "source deletion is not implicit policy deletion"
        );

        let main =
            "{ permissions = { sets = []; worker_default = {}; }; } as WorkspaceConfigSchema\n";
        std::fs::write(checkout.join("source/main.dcdl"), &main).unwrap();
        let preview = server.run(&f.owner, &checkout, Preview).await.unwrap();
        assert!(preview.contains("main.dcdl"), "{preview}");
        server.run(&f.owner, &checkout, Push).await.unwrap();
        let removed = f.source().await;
        f.assert_source_policy_unchanged(&removed);
        assert_eq!(
            shared_permissions(&removed).worker_default.assignment(),
            Default::default()
        );
        f.assert_subject_policy_unchanged();
        let repull = dir.path().join("repull");
        server.run(&f.owner, &repull, Pull).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(checkout.join("source/main.dcdl")).unwrap(),
            main,
            "adoption keeps the local editor's text"
        );
        assert_eq!(
            std::fs::read_to_string(repull.join("source/main.dcdl")).unwrap(),
            removed.state.snapshot.entries["main.dcdl"].content
        );
        assert!(!repull.join("source").join(MARKER).exists());
        assert!(!repull.join("source").join(&policy_path).exists());
    }

    #[tokio::test]
    async fn cli_tcp_stale_edit_rejects_concurrent_cli_and_browser_aba_without_losing_dirty_files()
    {
        use yoi::config_cli::Command::*;
        let f = Fixture::new().await;
        let server = CliServer::start(&f).await;
        let dir = tempfile::tempdir().unwrap();
        let writer = dir.path().join("writer");
        server.run(&f.owner, &writer, Pull).await.unwrap();
        let marker = writer.join("source").join(MARKER);
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(&marker, "A").unwrap();
        server.run(&f.owner, &writer, Push).await.unwrap();
        let a = f.source().await;
        let stale = dir.path().join("stale-editor");
        server.run(&f.owner, &stale, Pull).await.unwrap();
        let local = stale.join("source").join(MARKER);
        std::fs::write(&local, "unsaved local edit").unwrap();
        let metadata = stale.join(".yoi-config/state.json");
        let before_metadata = std::fs::read(&metadata).unwrap();

        std::fs::write(&marker, "B").unwrap();
        server.run(&f.owner, &writer, Push).await.unwrap();
        let b = f.source().await;
        let before = f.durable();
        for command in [Validate, Preview, Push] {
            let error = server.run(&f.owner, &stale, command).await.unwrap_err();
            assert_eq!(error.exit_code, 4, "{command:?}: {error}");
            assert_eq!(f.durable(), before);
            assert_eq!(std::fs::read(&metadata).unwrap(), before_metadata);
            assert_eq!(
                std::fs::read_to_string(&local).unwrap(),
                "unsaved local edit"
            );
        }
        let error = server.run(&f.owner, &stale, Pull).await.unwrap_err();
        assert_eq!(error.exit_code, 2, "dirty pull must refuse: {error}");
        assert_eq!(std::fs::read(&metadata).unwrap(), before_metadata);
        assert_eq!(
            std::fs::read_to_string(&local).unwrap(),
            "unsaved local edit"
        );
        assert_eq!(f.durable(), before);

        // An independently authenticated Web owner returns content to A over TCP.
        let returning = request(&b, "cli-aba-web-return-a", vec![update(&b, MARKER, "A")]);
        let response = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap()
            .post(format!("{}{}", server.origin, Fixture::path("/commit")))
            .header(reqwest::header::COOKIE, &f.cookie)
            .header(reqwest::header::ORIGIN, &f.origin)
            .json(&returning)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let receipt: Receipt = response.json().await.unwrap();
        let returned = f.source().await;
        assert_eq!(
            returned.state.snapshot, a.state.snapshot,
            "real content root returned to A"
        );
        assert_eq!(
            returned.history_head.as_deref(),
            Some(receipt.history_head.as_str())
        );
        assert_ne!(
            returned.history_head, a.history_head,
            "history did not return to A"
        );
        let before = f.durable();
        for command in [Preview, Push] {
            let error = server.run(&f.owner, &stale, command).await.unwrap_err();
            assert_eq!(error.exit_code, 4, "ABA {command:?}: {error}");
            assert_eq!(f.durable(), before);
        }
        assert_eq!(std::fs::read(&metadata).unwrap(), before_metadata);
        assert_eq!(
            std::fs::read_to_string(&local).unwrap(),
            "unsaved local edit"
        );
        let fresh = dir.path().join("resolved");
        server.run(&f.owner, &fresh, Pull).await.unwrap();
        std::fs::copy(&local, fresh.join("source").join(MARKER)).unwrap();
        server.run(&f.owner, &fresh, Push).await.unwrap();
        assert_eq!(
            f.source().await.state.snapshot.entries[MARKER].content,
            "unsaved local edit"
        );
    }

    #[tokio::test]
    async fn cli_tcp_invalid_source_is_validation_refusal_without_storage_or_local_loss() {
        use yoi::config_cli::Command::*;
        let f = Fixture::new().await;
        let server = CliServer::start(&f).await;
        let dir = tempfile::tempdir().unwrap();
        let checkout = dir.path().join("checkout");
        server.run(&f.owner, &checkout, Pull).await.unwrap();
        let draft = f.source().await;
        let path = permission_path(&draft);
        let local = checkout.join("source").join(&path);
        let original = std::fs::read(&local).unwrap();
        let metadata = checkout.join(".yoi-config/state.json");
        let metadata_before = std::fs::read(&metadata).unwrap();
        let before = f.durable();
        for (case, content) in [
            ("syntax", "{ subjects = [".to_owned()),
            (
                "unknown-key",
                permission_source("ticket.not_a_builtin_operation", "ticket.query"),
            ),
            ("unknown-set", "{ worker_default = { sets = [\"not-a-builtin-set\"]; }; }".to_owned()),
            ("key-payload", r#"{ worker_default = { namespaces = [{ namespace = "ticket"; keys = [{ key = "ticket.query"; arguments = {}; }]; }]; }; }"#.to_owned()),
            ("wrong-namespace", r#"{ worker_default = { namespaces = [{ namespace = "ticket"; keys = ["worker.observe"]; }]; }; }"#.to_owned()),
        ] {
            std::fs::write(&local, &content).unwrap();
            for command in [Validate, Preview, Push] {
                let error = server.run(&f.owner, &checkout, command).await.unwrap_err();
                assert_eq!(error.exit_code, 3, "{case} {command:?}: {error}");
                assert_eq!(
                    f.durable(),
                    before,
                    "{case} {command:?} must not modify source, policy or seal"
                );
                assert_eq!(std::fs::read_to_string(&local).unwrap(), content);
                assert_eq!(std::fs::read(&metadata).unwrap(), metadata_before);
                f.assert_subject_policy_unchanged();
            }
        }
        std::fs::write(&local, original).unwrap();
        server.run(&f.owner, &checkout, Push).await.unwrap();
        f.assert_native_writer_denied();
    }

    #[tokio::test]
    async fn cli_tcp_nonowner_is_refused_by_server_before_and_after_owner_adoption() {
        use server_api::client_support::ClientError;
        use yoi::config_cli::Command::*;
        let f = Fixture::new().await;
        let server = CliServer::start(&f).await;
        let dir = tempfile::tempdir().unwrap();
        for edited in [false, true] {
            let checkout = dir.path().join(format!("owner-{edited}"));
            server.run(&f.owner, &checkout, Pull).await.unwrap();
            let source = f.source().await;
            assert!(source.source_stored);
            let before = f.durable();
            let denied_checkout = dir.path().join(format!("foreign-{edited}"));
            let error = server
                .run(&f.foreign, &denied_checkout, Pull)
                .await
                .unwrap_err();
            assert_eq!(error.exit_code, 5, "{error}");
            assert!(!denied_checkout.exists());
            let metadata = std::fs::read(checkout.join(".yoi-config/state.json")).unwrap();
            for command in [Validate, Preview, Push] {
                let error = server
                    .run(&f.foreign, &checkout, command)
                    .await
                    .unwrap_err();
                assert_eq!(error.exit_code, 5, "{command:?}: {error}");
                assert_eq!(f.durable(), before);
                assert_eq!(
                    std::fs::read(checkout.join(".yoi-config/state.json")).unwrap(),
                    metadata
                );
            }
            // CLI push stops at preview. Also prove the actual commit/reconcile
            // boundaries refuse this same authenticated nonowner independently.
            let adoption = request(
                &source,
                &format!("cli-denied-{edited}"),
                vec![create_marker("forbidden")],
            );
            let foreign = server.client(&f.foreign);
            for result in [
                foreign
                    .configuration_adopt(TEST_WORKSPACE_ID.into(), adoption.clone())
                    .await
                    .map(|_| ()),
                foreign
                    .configuration_reconcile(TEST_WORKSPACE_ID.into(), adoption)
                    .await
                    .map(|_| ()),
            ] {
                match result.unwrap_err() {
                    ClientError::Public { status, .. } => assert_eq!(status.as_u16(), 403),
                    other => panic!("expected actual Server authorization refusal: {other:?}"),
                }
                assert_eq!(f.durable(), before);
            }
            f.assert_subject_policy_unchanged();
            server.run(&f.owner, &checkout, Push).await.unwrap();
            f.assert_native_writer_denied();
        }
    }

    fn core_request(request: &Adoption) -> crate::config_adoption::ConfigurationAdoptionRequest {
        crate::config_adoption::ConfigurationAdoptionRequest {
            operation_id: request.operation_id.clone(),
            expected_head: request.expected_head.clone(),
            expected_catalog_digest: request.expected_catalog_digest.clone(),
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
        assert!(
            source
                .state
                .snapshot
                .entries
                .contains_key("permissions/owner.dcdl")
        );
        "permissions/owner.dcdl".into()
    }

    fn shared_permissions(source: &Source) -> crate::policy_config::PermissionSource {
        let snapshot =
            serde_json::from_value(serde_json::to_value(&source.state.snapshot).unwrap()).unwrap();
        let contract =
            serde_json::from_value(serde_json::to_value(&source.state.contract).unwrap()).unwrap();
        let evaluated = config_source::SnapshotEnvironment::new(snapshot)
            .evaluate_contract(&contract)
            .unwrap();
        assert_eq!(evaluated.projection_digest, source.state.projection_digest);
        crate::policy_config::source(&evaluated).unwrap()
    }

    fn permission_source(default: &str, set_key: &str) -> String {
        format!(
            r#"{{
    sets = [{{ id = "owner-tools"; namespaces = [{{ namespace = "ticket"; keys = [{set_key:?}]; }}]; }}];
    worker_default = {{ namespaces = [{{ namespace = "ticket"; keys = [{default:?}]; }}]; }};
}}
"#
        )
    }

    // Uses the same catalog/owner/SQLite fixture, with only the HTTP transport
    // extended to loopback for the production generated client's decoder.
    struct AdoptionLoopback {
        origin: String,
        task: tokio::task::JoinHandle<()>,
    }
    impl AdoptionLoopback {
        async fn start(app: Router) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            Self { origin, task }
        }
    }
    impl Drop for AdoptionLoopback {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    #[tokio::test]
    async fn configuration_http_problem_preserves_catalog_conflict_owner_visibility_and_durable_state()
     {
        let f = Fixture::new().await;
        let draft = f.source().await;
        let before = f.durable();
        let mut adoption = request(
            &draft,
            "private-config-catalog-request",
            vec![create_marker("private-source-body")],
        );
        adoption.expected_catalog_digest.push_str("-private-stale");
        let request = f.bearer(
            &f.owner,
            "POST",
            "/commit",
            Some(serde_json::to_value(&adoption).unwrap()),
        );
        let response = f.app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(response.headers()[CONTENT_TYPE], "application/problem+json");
        let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
        let problem: Value = serde_json::from_slice(&bytes).unwrap();
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../web/workspace/test/config-source/fixtures/catalog-changed.json"
        ))
        .unwrap();
        assert_eq!(
            problem, fixture,
            "Web must consume the exact production HTTP fixture"
        );
        for private in [
            "private-config-catalog-request",
            "private-source-body",
            "private-stale",
            MARKER,
            f.subject.as_str(),
        ] {
            assert!(!String::from_utf8_lossy(&bytes).contains(private));
        }
        assert_eq!(f.durable(), before);
        f.assert_subject_policy_unchanged();
        let (status, denied) = f
            .call(f.bearer(
                &f.foreign,
                "POST",
                "/commit",
                Some(serde_json::to_value(&adoption).unwrap()),
            ))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(
            denied.get("yoi_failure").is_none(),
            "owner denial is not cause visibility"
        );
        assert_eq!(f.durable(), before);

        let server = AdoptionLoopback::start(f.app.clone()).await;
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", f.owner).parse().unwrap(),
        );
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap();
        let generated = server_api::ConfigurationAdoptionApiClient::builder(&server.origin)
            .unwrap()
            .client(client)
            .build()
            .unwrap();
        let received = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            generated.configuration_adopt(TEST_WORKSPACE_ID.to_owned(), adoption),
        )
        .await
        .unwrap()
        .unwrap_err();
        let server_api::client_support::ClientError::Public { status, error } = received else {
            panic!("generated client lost the real Problem response: {received:?}")
        };
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            error.details().unwrap().reason,
            server_api::configuration_failure::ConfigurationFailureReason::CatalogChanged
        );
        assert!(
            api_macros::HttpError::custom_response(&error).is_none(),
            "received facts cannot encode a new disclosure"
        );
        assert_eq!(f.durable(), before);
    }

    #[tokio::test]
    async fn owner_http_repository_validation_is_read_only_before_and_after_source_edit() {
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
        for edited in [false, true] {
            let current = f.source().await;
            assert!(current.source_stored);
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
                    "the configured repository was not found",
                ),
                (
                    "non-ssh",
                    "nonssh",
                    "deploy",
                    "matching",
                    "read_only",
                    "the access binding requires an SSH repository source",
                ),
                (
                    "credential",
                    "remote",
                    "absent",
                    "matching",
                    "read_only",
                    "the configured SSH credential was not found",
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
                    "the configured SSH host trust was not found",
                ),
                (
                    "host",
                    "remote",
                    "deploy",
                    "wrong-host",
                    "read_only",
                    "the repository SSH endpoint does not match the host trust",
                ),
                (
                    "port",
                    "remote",
                    "deploy",
                    "wrong-port",
                    "read_only",
                    "the repository SSH endpoint does not match the host trust",
                ),
            ] {
                let adoption = request(
                    &current,
                    &format!("repository-{edited}-{case}"),
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
                    f.assert_subject_policy_unchanged();
                }
            }
            let valid = request(
                &current,
                &format!("valid-repository-{edited}"),
                vec![update(
                    &current,
                    "main.dcdl",
                    &content(
                        "remote",
                        "deploy",
                        "matching",
                        if edited { "read_write" } else { "read_only" },
                    ),
                )],
            );
            let preview = f.owner_call("/preview", &valid, StatusCode::OK).await;
            assert_eq!(
                f.durable(),
                before,
                "valid preview must remain a read, before and after an owner source edit"
            );
            assert_eq!(secret_counts(), secrets_before);
            let saved = f.commit(&valid).await;
            assert_eq!(
                serde_json::to_value(&saved.state).unwrap(),
                preview["state"]
            );
            assert_eq!(f.source().await.state, saved.state);
            f.assert_native_writer_denied();
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
    async fn owner_http_shared_source_edit_commits_head_and_receipt_without_changing_subject_policy()
     {
        let fixture = Fixture::new().await;
        let before = fixture.durable();
        let source = fixture.source().await;
        assert!(source.source_stored);
        assert!(source.history_head.is_some());
        fixture.assert_source_policy_unchanged(&source);
        assert_eq!(
            source.state.snapshot.digest,
            before["state"]["snapshot"]["digest"]
        );
        assert_eq!(source.catalog.workspace.as_str(), TEST_WORKSPACE_ID);
        fixture.assert_subject_policy_unchanged();
        let path = permission_path(&source);
        let changed = permission_source("ticket.create", "ticket.query");
        let adoption = request(
            &source,
            "owner-shared-source-edit",
            vec![update(&source, &path, &changed)],
        );
        let preview: Preview = serde_json::from_value(
            fixture
                .owner_call("/preview", &adoption, StatusCode::OK)
                .await,
        )
        .unwrap();
        assert_eq!(preview.expected_head, source.history_head);
        assert_eq!(preview.changed_paths, vec![path.clone()]);
        assert!(preview.changed_subjects.is_empty());
        assert_eq!(
            fixture.durable(),
            before,
            "preview does not publish source or mutate Subject policy"
        );
        let (status, wire) = fixture
            .call(fixture.browser(
                "POST",
                "/commit",
                Some(serde_json::to_value(&adoption).unwrap()),
                Some(&fixture.origin),
            ))
            .await;
        assert_eq!(status, StatusCode::OK, "{wire}");
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
        assert_eq!(stored.state, receipt.state);
        assert_eq!(
            stored.history_head.as_deref(),
            Some(receipt.history_head.as_str())
        );
        assert_ne!(stored.history_head, source.history_head);
        assert_eq!(
            shared_permissions(&stored).worker_default.assignment(),
            assignment("ticket.create")
        );
        fixture.assert_source_policy_unchanged(&stored);
        fixture.assert_subject_policy_unchanged();
        let after = fixture.durable();
        assert_eq!(
            after["legacy_receipt"], before["legacy_receipt"],
            "ordinary owner edits never reseal native transfer"
        );
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
        assert_eq!(fixture.commit(&adoption).await, receipt);
        assert_eq!(
            fixture.durable(),
            after,
            "exact replay does not write a new receipt or policy identity"
        );
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
        fixture.assert_native_writer_denied();
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
        fixture.assert_subject_policy_unchanged();

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
        // Clearing shared defaults makes old-source resurrection observable.
        let removal = "{ sets = []; worker_default = {}; }\n";
        fixture
            .commit(&request(
                &current,
                "later-policy-removal",
                vec![update(&current, &path, &removal)],
            ))
            .await;
        fixture.assert_subject_policy_unchanged();
        let cleared = fixture.source().await;
        assert_eq!(
            shared_permissions(&cleared).worker_default.assignment(),
            Default::default()
        );
        fixture.assert_source_policy_unchanged(&cleared);
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
        fixture.assert_subject_policy_unchanged();
        fixture.assert_native_writer_denied();
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
    async fn http_owner_and_cookie_origin_denials_leave_source_subject_policy_and_seal_intact() {
        let fixture = Fixture::new().await;
        for edited in [false, true] {
            let source = fixture.source().await;
            assert!(source.source_stored);
            let change = if edited {
                update(&source, MARKER, "second authorized value")
            } else {
                create_marker("first authorized value")
            };
            let adoption = request(&source, &format!("denied-cutover-{edited}"), vec![change]);
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
                    fixture.assert_subject_policy_unchanged();
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
                        fixture.assert_subject_policy_unchanged();
                    }
                }
            }
            // A Browser GET does not require a mutation Origin, and the same
            // valid adoption remains possible after every denied request.
            let (status, response) = fixture.call(fixture.browser("GET", "", None, None)).await;
            assert_eq!(status, StatusCode::OK, "{response}");
            assert_eq!(serde_json::from_value::<Source>(response).unwrap(), source);
            fixture.commit(&adoption).await;
            fixture.assert_native_writer_denied();
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
                permission_source("ticket.not_a_builtin_operation", "ticket.query"),
            ),
            (
                "retired-subject-input",
                format!(
                    "{{ subjects = [{{ subject_id = {:?}; execution = {{}}; }}]; }}",
                    fixture.subject
                ),
            ),
            (
                "retired-removal-input",
                format!("{{ remove_subjects = [{:?}]; }}", fixture.subject),
            ),
            (
                "unknown-set",
                r#"{ worker_default = { sets = ["unknown-adoption-set"]; }; }"#.to_owned(),
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
                    "invalid {name} leaves source, policy, head, receipts and seal unchanged"
                );
                fixture.assert_subject_policy_unchanged();
            }
        }
        for edited in [false, true] {
            if edited {
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
                "old writer canonical={edited}: {response}"
            );
            assert_eq!(fixture.durable(), before);
            fixture.assert_subject_policy_unchanged();
        }
    }
    #[tokio::test]
    async fn publication_http_connects_adoption_dispatch_projection_runtime_and_read_only_lookup() {
        let fixture = Fixture::new().await;
        let draft = fixture.source().await;
        let adoption = request(&draft, "publication-http-success", vec![]);
        let receipt = fixture.commit(&adoption).await;
        let host = fixture.api.host_executors.host().unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(15), host.drain())
            .await
            .unwrap()
            .unwrap();
        let suffix = format!("/publication/{}", adoption.operation_id);
        let (status, before) = fixture
            .call(fixture.bearer(&fixture.owner, "GET", &suffix, None))
            .await;
        assert_eq!(status, StatusCode::OK, "{before}");
        let publication = &before["publication"];
        assert_eq!(publication["workspace_id"], TEST_WORKSPACE_ID);
        assert_eq!(publication["operation_id"], receipt.operation_id);
        assert_eq!(publication["history_head"], receipt.history_head);
        assert_eq!(publication["source_digest"], receipt.state.snapshot.digest);
        let observations = publication["observations"].as_array().unwrap();
        assert_eq!(observations[0]["stage"], "dispatch");
        assert_eq!(observations[0]["outcome"], "observed");
        assert_eq!(observations[1]["stage"], "projection");
        assert_eq!(observations[1]["outcome"], "observed");
        assert!(
            observations.len() >= 3,
            "real fixture Runtime must be observed"
        );
        for observation in &observations[2..] {
            assert_eq!(observation["stage"], "runtime");
            assert!(observation["runtime_id"].is_string());
            assert_eq!(
                observation["projection_digest"],
                observations[1]["projection_digest"]
            );
        }
        let durable = fixture.durable();
        assert_eq!(fixture.commit(&adoption).await, receipt);
        fixture
            .owner_call("/reconcile", &adoption, StatusCode::OK)
            .await;
        tokio::time::timeout(std::time::Duration::from_secs(15), host.drain())
            .await
            .unwrap()
            .unwrap();
        let (status, after) = fixture
            .call(fixture.bearer(&fixture.owner, "GET", &suffix, None))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            after, before,
            "lookup, reconcile and replay do not republish or amend observations"
        );
        assert_eq!(
            fixture.durable(),
            durable,
            "independent evidence must not rewrite source/policy/receipt"
        );
        let (status, missing) = fixture
            .call(fixture.bearer(&fixture.owner, "GET", "/publication/not-observed", None))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert!(missing["publication"].is_null());
        let (status, denied) = fixture
            .call(fixture.bearer(&fixture.foreign, "GET", &suffix, None))
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(!denied.to_string().contains("request_fingerprint"));
    }

    #[tokio::test]
    async fn publication_dispatch_refusal_preserves_adoption_and_reprojects_safe_cause_for_browser_and_tool()
     {
        let host = HostExecutors::default();
        host.host.set(None).ok().unwrap();
        let fixture = Fixture::with_publication_host(host).await;
        let draft = fixture.source().await;
        let adoption = request(&draft, "publication-host-unavailable", vec![]);
        let receipt = fixture.commit(&adoption).await;
        assert!(receipt.source_stored && receipt.policy_committed);
        let suffix = format!("/publication/{}", adoption.operation_id);
        let (status, body) = fixture
            .call(fixture.bearer(&fixture.owner, "GET", &suffix, None))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let observations = body["publication"]["observations"].as_array().unwrap();
        assert_eq!(
            observations.len(),
            1,
            "projection and Runtime were not dispatched"
        );
        assert_eq!(observations[0]["outcome"], "unconfirmed");
        let browser_cause = &observations[0]["failure"]["failure"];
        assert_eq!(browser_cause["reason"], "publication_dispatch_unavailable");
        assert!(
            browser_cause["explanation"]
                .as_str()
                .unwrap()
                .contains("adopted")
        );
        let retained = fixture
            .api
            .config_store
            .configuration_publication(
                TEST_WORKSPACE_ID,
                &fixture.owner_account,
                &adoption.operation_id,
            )
            .unwrap()
            .unwrap();
        let tool = retained.observations[0].failure.as_ref().unwrap()
            .project_for::<error_contract::Tool, server_api::configuration_failure::ConfigurationFailureDetails>(
                &server_api::configuration_failure::ConfigurationFailureVisibility { can_view_cause: true }).unwrap();
        assert_eq!(
            tool.explanation(),
            browser_cause["explanation"].as_str().unwrap()
        );
        assert_eq!(tool.details().reason, server_api::configuration_failure::ConfigurationFailureReason::PublicationDispatchUnavailable);
        assert_eq!(fixture.commit(&adoption).await, receipt);
        let (_, replayed) = fixture
            .call(fixture.bearer(&fixture.owner, "GET", &suffix, None))
            .await;
        assert_eq!(replayed, body);
    }

    #[tokio::test]
    async fn publication_query_never_forwards_received_explanation_unknown_or_corrupt_evidence() {
        let host = HostExecutors::default();
        host.host.set(None).ok().unwrap();
        let fixture = Fixture::with_publication_host(host).await;
        let receipt = fixture.adopt_draft("publication-untrusted-evidence").await;
        let facts = error_contract::FailureFacts::new(
            protocol::control_failure::RuntimeControlCause::TimedOut,
        )
        .unwrap();
        let projected =
            error_contract::project_failure::<error_contract::Storage, _>(&facts, &()).unwrap();
        let original =
            protocol::background_failure::BackgroundFailure::from_projected(&projected).unwrap();
        let mut known: Value = serde_json::from_str(original.as_storage_str()).unwrap();
        known["explanation"] = json!("TEST_ONLY_SECRET /private/path body; safe to resend");
        let mut future = known.clone();
        future["version"] = json!(999);
        for (runtime, evidence) in [
            ("known", known.to_string()),
            ("future", future.to_string()),
            ("corrupt", "{ TEST_ONLY_SECRET raw_provider_body".into()),
        ] {
            fixture.api.config_store.with_conn(|conn| {
                conn.execute("INSERT INTO workspace_configuration_publication_observations(workspace_id,operation_id,stage,runtime_id,projection_digest,outcome,failure_cause) VALUES(?1,?2,'runtime',?3,'observed-projection','unconfirmed',?4)", rusqlite::params![TEST_WORKSPACE_ID, receipt.operation_id, runtime, evidence])?;
                Ok(())
            }).unwrap();
        }
        let suffix = format!("/publication/{}", receipt.operation_id);
        let (status, body) = fixture
            .call(fixture.bearer(&fixture.owner, "GET", &suffix, None))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let text = body.to_string();
        for private in [
            "TEST_ONLY_SECRET",
            "/private/path",
            "raw_provider_body",
            "safe to resend",
        ] {
            assert!(!text.contains(private), "{text}");
        }
        let observations = body["publication"]["observations"].as_array().unwrap();
        for observation in &observations[1..] {
            assert_eq!(observation["outcome"], "unconfirmed");
            if observation["runtime_id"] == "known" {
                assert_eq!(observation["failure"]["failure"]["reason"], "timed_out");
            } else {
                assert_eq!(observation["failure"]["kind"], "unavailable");
            }
        }
        let durable = fixture.durable();
        fixture
            .api
            .config_store
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE workspaces SET state='deleting' WHERE workspace_id=?1",
                    [TEST_WORKSPACE_ID],
                )?;
                Ok(())
            })
            .unwrap();
        let (status, denied) = fixture
            .call(fixture.bearer(&fixture.owner, "GET", &suffix, None))
            .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(!denied.to_string().contains("request_fingerprint"));
        assert_eq!(durable["receipts"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn queued_publication_rechecks_current_head_before_resolving_or_notifying_old_source() {
        let host = HostExecutors::default();
        host.host
            .set(Some(
                crate::db_executor::DbExecutor::new(crate::db_executor::DbExecutorLimits {
                    workers: std::num::NonZeroUsize::new(1).unwrap(),
                    queued: std::num::NonZeroUsize::new(4).unwrap(),
                })
                .unwrap(),
            ))
            .ok()
            .unwrap();
        let fixture = Fixture::with_publication_host(host).await;
        let executor = fixture.api.host_executors.host().unwrap();
        let (ready, started) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        executor
            .dispatch(move || {
                let _ = ready.send(());
                let _ = blocked.recv_timeout(std::time::Duration::from_secs(15));
            })
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(15), started)
            .await
            .unwrap()
            .unwrap();
        let first = fixture.adopt_draft("publication-queued-first").await;
        let second = fixture.adopt_draft("publication-queued-second").await;
        assert_ne!(first.history_head, second.history_head);
        release.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(15), executor.drain())
            .await
            .unwrap()
            .unwrap();
        let suffix = format!("/publication/{}", first.operation_id);
        let (status, body) = fixture
            .call(fixture.bearer(&fixture.owner, "GET", &suffix, None))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let observations = body["publication"]["observations"].as_array().unwrap();
        assert_eq!(
            observations.len(),
            2,
            "historical source must not be sent to any Runtime"
        );
        assert_eq!(observations[0]["outcome"], "observed");
        assert_eq!(observations[1]["stage"], "projection");
        assert_eq!(observations[1]["outcome"], "unconfirmed");
        assert!(observations[1]["projection_digest"].is_null());
        assert_eq!(
            observations[1]["failure"]["failure"]["reason"],
            "editing_head_changed"
        );
        assert_eq!(body["publication"]["history_head"], first.history_head);
        assert_eq!(
            fixture.source().await.history_head,
            Some(second.history_head)
        );
    }

    #[tokio::test]
    async fn publication_registry_failure_preserves_committed_receipt_and_never_resends_on_lookup_or_replay()
     {
        let host = HostExecutors::default();
        host.host
            .set(Some(
                crate::db_executor::DbExecutor::new(crate::db_executor::DbExecutorLimits {
                    workers: std::num::NonZeroUsize::new(1).unwrap(),
                    queued: std::num::NonZeroUsize::new(4).unwrap(),
                })
                .unwrap(),
            ))
            .ok()
            .unwrap();
        let fixture = Fixture::with_publication_host(host).await;
        let executor = fixture.api.host_executors.host().unwrap();
        let (ready, started) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        executor
            .dispatch(move || {
                let _ = ready.send(());
                blocked
                    .recv_timeout(std::time::Duration::from_secs(15))
                    .unwrap();
            })
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(15), started)
            .await
            .unwrap()
            .unwrap();
        let draft = fixture.source().await;
        let adoption = request(&draft, "publication-registry-unavailable", vec![]);
        let receipt = fixture.commit(&adoption).await;
        assert!(receipt.source_stored && receipt.policy_committed);
        let durable = fixture.durable();
        let registry = fixture.api.permission_registry.clone();
        assert!(
            std::thread::spawn(move || {
                let _guard = registry.write().unwrap();
                panic!("TEST_ONLY_SECRET /private/registry-path");
            })
            .join()
            .is_err()
        );
        release.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(15), executor.drain())
            .await
            .unwrap()
            .unwrap();
        let suffix = format!("/publication/{}", adoption.operation_id);
        let (status, before) = fixture
            .call(fixture.bearer(&fixture.owner, "GET", &suffix, None))
            .await;
        assert_eq!(status, StatusCode::OK, "{before}");
        let observations = before["publication"]["observations"].as_array().unwrap();
        assert_eq!(
            observations.len(),
            2,
            "a failed registry refresh must not notify Runtime"
        );
        assert_eq!(observations[0]["stage"], "dispatch");
        assert_eq!(observations[0]["outcome"], "observed");
        assert_eq!(observations[1]["stage"], "projection");
        assert_eq!(observations[1]["outcome"], "unconfirmed");
        let browser = &observations[1]["failure"]["failure"];
        assert_eq!(browser["reason"], "publication_registry_unavailable");
        assert!(browser["explanation"].as_str().unwrap().contains("adopted"));
        assert!(!before.to_string().contains("TEST_ONLY_SECRET"));
        assert!(!before.to_string().contains("/private/registry-path"));
        let retained = fixture
            .api
            .config_store
            .configuration_publication(
                TEST_WORKSPACE_ID,
                &fixture.owner_account,
                &adoption.operation_id,
            )
            .unwrap()
            .unwrap();
        let tool = retained.observations[1].failure.as_ref().unwrap()
            .project_for::<error_contract::Tool, server_api::configuration_failure::ConfigurationFailureDetails>(
                &server_api::configuration_failure::ConfigurationFailureVisibility { can_view_cause: true }).unwrap();
        assert_eq!(tool.explanation(), browser["explanation"].as_str().unwrap());
        assert_eq!(fixture.commit(&adoption).await, receipt);
        fixture
            .owner_call("/reconcile", &adoption, StatusCode::OK)
            .await;
        tokio::time::timeout(std::time::Duration::from_secs(15), executor.drain())
            .await
            .unwrap()
            .unwrap();
        let (status, after) = fixture
            .call(fixture.bearer(&fixture.owner, "GET", &suffix, None))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(after, before);
        assert_eq!(fixture.durable(), durable);
    }
}
