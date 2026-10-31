//! Source for the authenticated Subject lifecycle adapter contract. Router/Account,
//! Subject SQLite and ordinary create reservations are real; no model or product
//! process is started. Runtime execution and restore/stop callbacks are exercised
//! by the connected suites, not inferred from this projection/admission fixture.
use super::*;

pub(super) fn assert_failed_request(body: &Value, subject: &str, request: &str, resolution: &str) {
    assert!(body["error"].is_string(), "{body}");
    assert!(body["message"].is_string(), "{body}");
    assert!(body["diagnostics"].is_array(), "{body}");
    assert!(
        body.get("status").is_none(),
        "HTTP status is not a body field: {body}"
    );
    serde_json::from_value::<server_api::SubjectBodyApiError>(body.clone())
        .expect("the actual HTTP error must satisfy the dedicated typed shape");
    assert_eq!(
        body["request"],
        json!({
            "workspace_id": TEST_WORKSPACE_ID,
            "subject_id": subject,
            "request_id": request,
            "resolution": resolution,
        })
    );
}

struct BodyFixture {
    api: WorkspaceApi,
    backend: Arc<DeterministicExecutionBackend>,
    owner: String,
    subject: String,
    _dir: tempfile::TempDir,
}

impl BodyFixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (api, backend) = test_api_with_recording_backend(dir.path()).await;
        let owner = seed_test_workspace_owner_token(api.store.as_ref(), "body-owner").await;
        let subject = open_subjektiv_store(&api)
            .unwrap()
            .create_subject(crate::subjektiv::SubjectRole::new("companion").unwrap())
            .unwrap()
            .id;
        Self {
            api,
            backend,
            owner,
            subject,
            _dir: dir,
        }
    }
    fn path(&self) -> String {
        format!(
            "/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{}/body",
            self.subject
        )
    }
    async fn read(&self) -> Value {
        let (status, body) = response(
            &self.api,
            human_request(&self.owner, "GET", &self.path(), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }
    fn failed_reservation(&self) -> crate::store::WorkerSingletonLease {
        let key = crate::subjektiv::subject_worker_singleton_key(&self.subject).unwrap();
        let reservation = self
            .api
            .config_store
            .reserve_worker_create(
                TEST_WORKSPACE_ID,
                EMBEDDED_RUNTIME_ID,
                "failed-body-create",
                &"f".repeat(64),
                Some(&key),
                &self
                    .api
                    .config_store
                    .get_workspace_memory_settings(TEST_WORKSPACE_ID)
                    .unwrap(),
            )
            .unwrap();
        self.api
            .config_store
            .fail_worker_create_reservation(
                TEST_WORKSPACE_ID,
                EMBEDDED_RUNTIME_ID,
                reservation.worker_id,
                &reservation.create_fingerprint,
            )
            .unwrap();
        reservation.singleton.unwrap()
    }
    fn reserve(&self) -> crate::store::WorkerSingletonLease {
        let key = crate::subjektiv::subject_worker_singleton_key(&self.subject).unwrap();
        self.api
            .config_store
            .reserve_worker_create(
                TEST_WORKSPACE_ID,
                "runtime-test",
                "body-start-reservation",
                &"f".repeat(64),
                Some(&key),
                &self
                    .api
                    .config_store
                    .get_workspace_memory_settings(TEST_WORKSPACE_ID)
                    .unwrap(),
            )
            .unwrap()
            .singleton
            .unwrap()
    }
}

#[tokio::test]
async fn browser_subject_and_conversation_reads_allow_missing_origin_but_mutations_do_not() {
    let f = BodyFixture::new().await;
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::AUTHORIZATION,
        format!("Bearer {}", f.owner).parse().unwrap(),
    );
    let actor = resolve_actor(&ServerAuthApi::from(&f.api), &headers)
        .await
        .unwrap()
        .unwrap();
    let token = "subject-read-browser-session";
    f.api
        .store
        .create_browser_session(&BrowserSessionRecord {
            token_hash: crate::auth::token_hash(token),
            session_id: "subject-read-session".into(),
            user_id: actor.user_id,
            created_at: now_registry_timestamp(),
            expires_at: "2099-01-01T00:00:00Z".into(),
            revoked_at: None,
        })
        .unwrap();
    let AuthConfig::Passkey {
        cookie_name,
        origin,
        ..
    } = &f.api.config.auth;
    let browser_request = |method: &str, path: &str, body: Option<Value>, origin: Option<&str>| {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(axum::http::header::COOKIE, format!("{cookie_name}={token}"))
            .header(CONTENT_TYPE, "application/json");
        if let Some(origin) = origin {
            request = request.header(ORIGIN, origin);
        }
        request
            .body(Body::from(body.map(|v| v.to_string()).unwrap_or_default()))
            .unwrap()
    };
    let conversations = format!("/api/w/{TEST_WORKSPACE_ID}/chat/conversations");
    let create = json!({"operation_id":"browser-read-conversation", "subject_id":f.subject});
    let (status, conversation) = response(
        &f.api,
        browser_request("POST", &conversations, Some(create.clone()), Some(origin)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{conversation}");
    let messages = format!(
        "{conversations}/{}/messages",
        conversation["id"].as_str().unwrap()
    );
    for path in [f.path(), messages.clone()] {
        for origin in [None, Some(origin.as_str())] {
            let (status, body) =
                response(&f.api, browser_request("GET", &path, None, origin)).await;
            assert_eq!(status, StatusCode::OK, "{path}: {body}");
        }
        let (status, body) = response(
            &f.api,
            browser_request("GET", &path, None, Some("https://foreign.example")),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    }
    let expected =
        json!({"runtime_id":"runtime-test", "worker_id":"not-started", "generation":"1"});
    for (path, body) in [
        (conversations, create),
        (
            messages.clone(),
            json!({"operation_id":"must-not-send", "text":"must not be saved"}),
        ),
        (
            format!("{}/start", f.path()),
            json!({"request_id":"must-not-start", "expected_current":null, "expected_observation_token":null}),
        ),
        (
            format!("{}/recover", f.path()),
            json!({"request_id":"must-not-recover", "expected_current":expected}),
        ),
        (
            format!("{}/stop", f.path()),
            json!({"request_id":"must-not-stop", "expected_current":expected, "expected_observation_token":"not-observed"}),
        ),
    ] {
        for origin in [None, Some("https://foreign.example")] {
            let (status, error) = response(
                &f.api,
                browser_request("POST", &path, Some(body.clone()), origin),
            )
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{path}: {error}");
        }
    }
    let (status, page) = response(&f.api, browser_request("GET", &messages, None, None)).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["messages"], json!([]));
    assert!(f.read().await["current"].is_null());
    f.api
        .store
        .revoke_browser_session(&crate::auth::token_hash(token), &now_registry_timestamp())
        .unwrap();
    for path in [f.path(), messages] {
        let (status, body) = response(&f.api, browser_request("GET", &path, None, None)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    }
}

#[tokio::test]
async fn body_read_retains_an_occupied_owner_when_runtime_observation_is_unavailable() {
    let f = BodyFixture::new().await;
    let absent = f.read().await;
    assert!(absent["current"].is_null());
    assert_eq!(absent["observation"]["kind"], "absent");
    assert_eq!(absent["can_start"], true);
    let lease = f.reserve();
    let occupied = f.read().await;
    assert_eq!(occupied["current"]["worker_id"], lease.worker.worker_id);
    assert_eq!(
        occupied["current"]["generation"],
        lease.generation.to_string()
    );
    assert_eq!(occupied["observation"]["kind"], "unavailable");
    let (status, subject) = response(
        &f.api,
        human_request(
            &f.owner,
            "GET",
            f.path().strip_suffix("/body").unwrap(),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{subject}");
    assert_eq!(subject["current_body"], occupied["current"]);
    assert!(
        !subject["current_worker_diagnostics"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn body_operations_reject_nonowners_and_cross_workspace_targets() {
    let f = BodyFixture::new().await;
    let reader = seed_test_api_token(f.api.store.as_ref(), "body-reader");
    for (method, suffix, payload) in [
        (
            "POST",
            "/recover",
            Some(
                json!({"request_id":"forbidden-recovery", "expected_current":{"runtime_id":"r", "worker_id":"w", "generation":"1"}}),
            ),
        ),
        ("GET", "", None),
        (
            "POST",
            "/start",
            Some(
                json!({"request_id":"start", "expected_current":null, "expected_observation_token":null}),
            ),
        ),
    ] {
        let (status, body) = response(
            &f.api,
            human_request(&reader, method, &format!("{}{suffix}", f.path()), payload),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert!(
            body["request"].is_null(),
            "authorization before action admission supplies no request resolution: {body}"
        );
    }
    let path = f.path().replace(TEST_WORKSPACE_ID, "another-workspace");
    let (status, body) = response(&f.api, human_request(&f.owner, "GET", &path, None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(f.read().await["current"].is_null());
}

#[tokio::test]
async fn body_stop_and_outcome_reject_a_target_from_another_subject_before_runtime_dispatch() {
    let f = BodyFixture::new().await;
    let lease = f.reserve();
    let permissions = open_subjektiv_store(&f.api)
        .unwrap()
        .subject_permissions(&f.subject)
        .unwrap();
    let other = open_subjektiv_store(&f.api)
        .unwrap()
        .create_subject(crate::subjektiv::SubjectRole::new("other-companion").unwrap())
        .unwrap();
    let path = format!(
        "/api/w/{TEST_WORKSPACE_ID}/subjektiv/subjects/{}/body",
        other.id
    );
    let request = json!({"request_id":"old-screen", "expected_current":subject_body::reference(&lease), "expected_observation_token":"old-token"});
    for suffix in ["/stop", "/stop/outcome"] {
        let (status, body) = response(
            &f.api,
            human_request(
                &f.owner,
                "POST",
                &format!("{path}{suffix}"),
                Some(request.clone()),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
    }
    assert_eq!(
        f.read().await["current"]["worker_id"],
        lease.worker.worker_id
    );
    assert_eq!(
        open_subjektiv_store(&f.api)
            .unwrap()
            .subject_permissions(&f.subject)
            .unwrap(),
        permissions
    );
}

#[tokio::test]
async fn body_mutations_reject_an_old_generation_without_releasing_its_owner() {
    let f = BodyFixture::new().await;
    let lease = f.reserve();
    let mut old = subject_body::reference(&lease);
    old.generation = (lease.generation + 1).to_string();
    for (suffix, payload) in [
        (
            "/recover",
            json!({"request_id":"old-recover", "expected_current":old}),
        ),
        (
            "/start",
            json!({"request_id":"old-start", "expected_current":old, "expected_observation_token":"old-token"}),
        ),
        (
            "/stop",
            json!({"request_id":"old-stop", "expected_current":old, "expected_observation_token":"old-token"}),
        ),
        (
            "/stop/outcome",
            json!({"request_id":"old-stop", "expected_current":old, "expected_observation_token":"old-token"}),
        ),
    ] {
        let (status, body) = response(
            &f.api,
            human_request(
                &f.owner,
                "POST",
                &format!("{}{suffix}", f.path()),
                Some(payload.clone()),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_failed_request(
            &body,
            &f.subject,
            payload["request_id"].as_str().unwrap(),
            "superseded",
        );
    }
    assert_eq!(
        f.read().await["current"],
        serde_json::to_value(subject_body::reference(&lease)).unwrap()
    );
}

#[tokio::test]
async fn body_operations_reject_a_subject_with_deletion_already_admitted() {
    let f = BodyFixture::new().await;
    let lease = f.reserve();
    open_subjektiv_store(&f.api)
        .unwrap()
        .begin_subject_deletion(&f.subject, "delete-body-fixture")
        .unwrap();
    let stop = json!({"request_id":"stop", "expected_current":subject_body::reference(&lease), "expected_observation_token":"observed"});
    for (method, suffix, payload) in [
        ("GET", "", None),
        (
            "POST",
            "/start",
            Some(
                json!({"request_id":"start", "expected_current":null, "expected_observation_token":null}),
            ),
        ),
        (
            "POST",
            "/recover",
            Some(
                json!({"request_id":"recover-deleted", "expected_current":subject_body::reference(&lease)}),
            ),
        ),
        ("POST", "/stop", Some(stop.clone())),
        ("POST", "/stop/outcome", Some(stop)),
    ] {
        let (status, body) = response(
            &f.api,
            human_request(&f.owner, method, &format!("{}{suffix}", f.path()), payload),
        )
        .await;
        assert!(
            matches!(status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
            "{status}: {body}"
        );
    }
    let deletion = open_subjektiv_store(&f.api)
        .unwrap()
        .subject_deletion(&f.subject)
        .unwrap()
        .unwrap();
    assert_eq!(deletion.operation_id, "delete-body-fixture");
    assert!(!deletion.completed);
    assert_eq!(
        f.api
            .store
            .current_worker_singleton_owner(TEST_WORKSPACE_ID, &lease.key)
            .unwrap(),
        Some(lease)
    );
}

#[tokio::test]
async fn body_mutation_json_requires_explicit_identity_and_fences() {
    let f = BodyFixture::new().await;
    for (suffix, payload) in [
        ("/start", json!({"request_id":"start"})),
        (
            "/stop",
            json!({"request_id":"stop", "expected_observation_token":"observed"}),
        ),
        ("/stop/outcome", json!({"expected_current":null})),
    ] {
        let (status, body) = response(
            &f.api,
            human_request(
                &f.owner,
                "POST",
                &format!("{}{suffix}", f.path()),
                Some(payload),
            ),
        )
        .await;
        assert!(status.is_client_error(), "{status}: {body}");
    }
    assert!(f.read().await["current"].is_null());
}

#[tokio::test]
async fn body_action_payload_rejection_cannot_claim_request_resolution() {
    let f = BodyFixture::new().await;
    for suffix in ["/start", "/stop", "/stop/outcome"] {
        let (status, body) = response(
            &f.api,
            human_request(
                &f.owner,
                "POST",
                &format!("{}{suffix}", f.path()),
                Some(json!({"request_id":"unbound-malformed"})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert!(
            body["request"].is_null(),
            "unparsed intent is not evidence: {body}"
        );
    }
}

#[tokio::test]
async fn failed_body_reservation_can_be_recovered_without_starting_a_worker() {
    let f = BodyFixture::new().await;
    let lease = f.failed_reservation();
    let before = f.read().await;
    assert_eq!(before["observation"]["kind"], "failed_create");
    assert!(
        f.backend.recovery_requests.lock().unwrap().is_empty(),
        "GET must not recover"
    );
    let request = json!({"request_id":"recover-failed", "expected_current":before["current"]});
    *f.backend.recovery_failure.lock().unwrap() = Some("Runtime absence unconfirmed".into());
    let (status, unknown) = response(
        &f.api,
        human_request(
            &f.owner,
            "POST",
            &format!("{}/recover", f.path()),
            Some(request.clone()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{unknown}");
    assert_eq!(unknown["disposition"], "unknown");
    assert_eq!(f.read().await["current"], before["current"]);
    let (status, recovered) = response(
        &f.api,
        human_request(
            &f.owner,
            "POST",
            &format!("{}/recover", f.path()),
            Some(request.clone()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{recovered}");
    assert_eq!(recovered["disposition"], "accepted");
    assert_eq!(recovered["body"]["observation"]["kind"], "absent");
    let requests = f.backend.recovery_requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
    assert!(
        f.api
            .runtime
            .list_workers_for_runtime(EMBEDDED_RUNTIME_ID, 100)
            .unwrap()
            .items
            .is_empty()
    );
    let successor = f.reserve();
    assert_ne!(successor.worker, lease.worker);
    let (status, stale) = response(
        &f.api,
        human_request(
            &f.owner,
            "POST",
            &format!("{}/recover", f.path()),
            Some(request),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{stale}");
    assert_failed_request(&stale, &f.subject, "recover-failed", "superseded");
    assert_eq!(
        f.read().await["current"],
        serde_json::to_value(subject_body::reference(&successor)).unwrap()
    );
    assert_eq!(f.backend.recovery_requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn body_recovery_rejects_unfinished_or_published_creation() {
    for publication in ["reserved", "writer", "session"] {
        let f = BodyFixture::new().await;
        let lease = if publication == "reserved" {
            f.reserve()
        } else {
            f.failed_reservation()
        };
        let store = open_subjektiv_store(&f.api).unwrap();
        if publication == "writer" {
            store
                .claim_journal_writer(
                    &f.subject,
                    &lease.worker.runtime_id,
                    &lease.worker.worker_id,
                )
                .unwrap();
        } else if publication == "session" {
            store
                .record_session_attribution(
                    crate::subjektiv::SubjectSessionAttribution::new(
                        &f.subject,
                        &lease.worker.runtime_id,
                        &lease.worker.worker_id,
                        session_store::new_session_id().to_string(),
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        let before = f.read().await;
        assert_eq!(before["observation"]["kind"], "unavailable");
        let request =
            json!({"request_id":"must-not-recover", "expected_current":before["current"]});
        let (status, error) = response(
            &f.api,
            human_request(
                &f.owner,
                "POST",
                &format!("{}/recover", f.path()),
                Some(request),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{error}");
        assert_eq!(f.read().await["current"], before["current"]);
        assert!(f.backend.recovery_requests.lock().unwrap().is_empty());
    }
}
