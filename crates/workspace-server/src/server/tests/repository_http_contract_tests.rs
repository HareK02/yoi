//! Real authenticated loopback router, registry/SQLite, generated Rust client and
//! opt-in production Web adapter. Only Worker execution is deterministic; these
//! tests do not launch a model or a live product process.
use super::*;
use server_api::client_support::ClientError;
use server_api::repository_failure::{
    RepositoryFailureDetails as Details, RepositoryFailureOperation as Op,
    RepositoryFailureReason as Reason, RepositoryFailureVisibility,
};

struct Fixture {
    api: WorkspaceApi,
    origin: String,
    http: reqwest::Client,
    client: server_api::ServerApiClient,
    token: String,
    _server: AuthorizationCallbackServer,
    _temp: tempfile::TempDir,
}
impl Fixture {
    async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let mut config = test_server_config(temp.path());
        config.backend_base_url = Some(origin.clone());
        config.repositories = vec![ConfiguredRepository {
            id: "files".into(),
            repository_key: "files".into(),
            provider: "local_fs".into(),
            source: server_api::RepositorySource {
                kind: server_api::RepositorySourceKind::LocalPath,
                uri: "/private/repository-canary".into(),
            },
            source_fingerprint: "sha256:test".into(),
            observed_status: server_api::RepositoryObservedStatus::Unverified,
            observed_at: None,
            path: None,
            default_selector: None,
        }];
        let store = Arc::new(test_control_store(&config));
        let token = seed_test_api_token(store.as_ref(), TEST_WORKSPACE_ID);
        let api = WorkspaceApi::new_with_execution_backend(
            config,
            store,
            Arc::new(DeterministicExecutionBackend::default()),
        )
        .await
        .unwrap();
        let server = AuthorizationCallbackServer::start(listener, build_router(api.clone()));
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap();
        let client = server_api::ServerApiClient::with_client(
            origin.as_str().try_into().unwrap(),
            http.clone(),
        );
        Self {
            api,
            origin,
            http,
            client,
            token,
            _server: server,
            _temp: temp,
        }
    }
    async fn problem(&self, path: &str, expected: Details, status: u16) {
        let response = self
            .http
            .get(format!("{}{path}", self.origin))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), status);
        assert_eq!(response.headers()[CONTENT_TYPE], "application/problem+json");
        let bytes = response.bytes().await.unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("canary"));
        let decoded = api_macros::http::decode_problem_response::<Details>(
            &bytes,
            status,
            "application/problem+json",
            &server_api::RepositoryBoundary,
            error_contract::Budget::new(16384, 16384),
        );
        let error_contract::DecodeFailure::Known(cause) = decoded else {
            panic!("HTTP cause lost")
        };
        assert_eq!(cause.facts().cause(), &expected);
    }
}
fn public(error: ClientError<server_api::RepositoryApiError>, expected: Details, status: u16) {
    let ClientError::Public {
        status: actual,
        error,
    } = error
    else {
        panic!("generated client lost Repository error")
    };
    assert_eq!(actual, status);
    assert_eq!(error.repository_cause(), Some(&expected));
    // A received cause must not acquire a Browser encoder capability.
    assert!(api_macros::HttpError::custom_response(&error).is_none());
    let facts = error_contract::FailureFacts::new(*error.repository_cause().unwrap()).unwrap();
    let visible = RepositoryFailureVisibility {
        can_view_cause: true,
    };
    let tool =
        error_contract::project_failure::<error_contract::Tool, _>(&facts, &visible).unwrap();
    assert_eq!(tool.details(), &expected);
    assert!(!tool.explanation().contains("canary"));
    assert!(
        error_contract::project_failure::<error_contract::Tool, _>(
            &facts,
            &RepositoryFailureVisibility::default()
        )
        .is_err()
    );
}

pub(super) async fn assert_lookup_and_registration_contract() {
    let f = Fixture::new().await;
    for (key, reason, status) in [
        ("missing-canary", Reason::UnknownRepository, 404),
        ("files", Reason::UnsupportedProvider, 400),
    ] {
        let expected = Details {
            operation: Op::RecentLog,
            reason,
        };
        for path in [
            format!("/api/repositories/{key}/log"),
            format!("/api/w/{TEST_WORKSPACE_ID}/repositories/{key}/log"),
        ] {
            f.problem(&path, expected, status).await;
        }
        public(
            f.client
                .repository_log(
                    TEST_WORKSPACE_ID.into(),
                    key.into(),
                    server_api::RepositoryLogQuery { limit: None },
                )
                .await
                .unwrap_err(),
            expected,
            status,
        );
        public(
            f.client
                .repository_log_alias(key.into(), server_api::RepositoryLogQuery { limit: None })
                .await
                .unwrap_err(),
            expected,
            status,
        );
    }
    let expected = Details {
        operation: Op::SummaryByKey,
        reason: Reason::UnknownRepository,
    };
    for path in [
        "/api/repositories/missing-canary".to_owned(),
        format!("/api/w/{TEST_WORKSPACE_ID}/repositories/missing-canary"),
    ] {
        f.problem(&path, expected, 404).await;
    }
    public(
        f.client
            .repository_detail(TEST_WORKSPACE_ID.into(), "missing-canary".into())
            .await
            .unwrap_err(),
        expected,
        404,
    );
    public(
        f.client
            .repository_detail_alias("missing-canary".into())
            .await
            .unwrap_err(),
        expected,
        404,
    );

    let request = server_api::CreateWorkspaceRepositoryRequest {
        repository_key: "registered".into(),
        source: "/private/original-canary".into(),
        default_ref: Some("main".into()),
    };
    assert!(
        !f.client
            .repository_create(TEST_WORKSPACE_ID.into(), request.clone())
            .await
            .unwrap()
            .replayed
    );
    let saved = f
        .api
        .config_store
        .get_repository_by_key(TEST_WORKSPACE_ID, "registered")
        .unwrap()
        .unwrap();
    let count = f
        .api
        .config_store
        .list_repositories(TEST_WORKSPACE_ID)
        .unwrap()
        .len();
    assert!(
        f.client
            .repository_create(TEST_WORKSPACE_ID.into(), request.clone())
            .await
            .unwrap()
            .replayed
    );
    let mut conflict = request.clone();
    conflict.source = "/private/different-canary".into();
    public(
        f.client
            .repository_create(TEST_WORKSPACE_ID.into(), conflict.clone())
            .await
            .unwrap_err(),
        Details {
            operation: Op::Register,
            reason: Reason::RegistrationIntentConflict,
        },
        409,
    );
    let response = f
        .http
        .post(format!(
            "{}/api/w/{TEST_WORKSPACE_ID}/repositories",
            f.origin
        ))
        .json(&conflict)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(response.headers()[CONTENT_TYPE], "application/problem+json");
    let body = response.text().await.unwrap();
    assert!(!body.contains("canary"));
    let wire: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        wire["yoi_failure"]["details"],
        json!({"operation":"register", "reason":"registration_intent_conflict"})
    );
    let after = f
        .api
        .config_store
        .get_repository_by_key(TEST_WORKSPACE_ID, "registered")
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(saved).unwrap(),
        serde_json::to_value(after).unwrap()
    );
    assert_eq!(
        f.api
            .config_store
            .list_repositories(TEST_WORKSPACE_ID)
            .unwrap()
            .len(),
        count
    );
    let native = ApiError::from(Error::RepositoryRegistrationConflict {
        repository_key: "private-canary".into(),
    })
    .into_repository_api_error();
    let attachment = native.failure.unwrap();
    let tool = attachment
        .project_for::<error_contract::Tool, Details>(&RepositoryFailureVisibility {
            can_view_cause: true,
        })
        .unwrap();
    assert_eq!(
        tool.details(),
        &Details {
            operation: Op::Register,
            reason: Reason::RegistrationIntentConflict
        }
    );
    assert!(
        attachment
            .project_for::<error_contract::Tool, Details>(&RepositoryFailureVisibility::default())
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires Deno; explicit repository real-Web receiver roundtrip"]
async fn repository_http_web_receives_lookup_and_registration_causes_without_losing_intent() {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;
    let f = Fixture::new().await;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut child = tokio::process::Command::new("deno")
        .current_dir(root.join("web/workspace"))
        .args([
            "run",
            "--cached-only",
            "--config",
            "test/drive-real-api/deno.json",
        ])
        .arg(format!(
            "--allow-net={}",
            f.origin.trim_start_matches("http://")
        ))
        .arg("test/repository-real-api/roundtrip.ts")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let wire = serde_json::to_vec(
        &json!({"origin":f.origin, "workspace_id":TEST_WORKSPACE_ID, "token":f.token}),
    )
    .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let output = tokio::time::timeout(std::time::Duration::from_secs(60), async move {
        stdin.write_all(&wire).await.unwrap();
        drop(stdin);
        child.wait_with_output().await.unwrap()
    })
    .await
    .expect("Repository Web receiver roundtrip timed out");
    let redact = |b: &[u8]| String::from_utf8_lossy(b).replace(&f.token, "[redacted]");
    assert!(
        output.status.success(),
        "Web receiver failed: {} {}",
        redact(&output.stdout),
        redact(&output.stderr)
    );
    let saved = f
        .api
        .config_store
        .get_repository_by_key(TEST_WORKSPACE_ID, "web-registration")
        .unwrap()
        .unwrap();
    assert_eq!(saved.source.uri, "/private/original-canary");
    assert_eq!(saved.default_ref.as_deref(), Some("main"));
}
