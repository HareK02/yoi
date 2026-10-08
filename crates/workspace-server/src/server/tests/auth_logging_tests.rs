use super::*;
use std::io::Write;
use tracing::instrument::WithSubscriber;
use worker_runtime::auth::{
    RUNTIME_REQUEST_SOURCE_PROOF_HEADER, RuntimeRequestSourceSigner, WORKSPACE_REQUEST_PERMISSION,
};

#[derive(Clone, Default)]
struct CapturedAuthLog(Arc<std::sync::Mutex<Vec<u8>>>);

impl Write for CapturedAuthLog {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl CapturedAuthLog {
    fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync + use<> {
        let writer = self.clone();
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .without_time()
            .with_ansi(false)
            .json()
            .flatten_event(true)
            .with_writer(move || writer.clone())
            .finish()
    }
    fn event(&self) -> (Value, String) {
        let text = String::from_utf8(self.0.lock().unwrap().clone()).unwrap();
        let events: Vec<Value> = text
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|event| event["event"] == "runtime_request_proof_rejected")
            .collect();
        assert_eq!(
            events.len(),
            1,
            "missing or duplicated auth diagnostic: {text}"
        );
        (events[0].clone(), text)
    }
}

#[tokio::test]
async fn rejected_runtime_proofs_log_reason_and_verification_without_secrets_on_both_routers() {
    let workspace = tempfile::tempdir().unwrap();
    let mut api = test_api(workspace.path()).await;
    let identity = RuntimeIdentityMaterial::generate("runtime-test").unwrap();
    configure_runtime_request_auth(&mut api, &identity, "runtime-test");
    let revoked = RuntimeIdentityMaterial::generate("revoked-runtime-secret").unwrap();
    configure_runtime_request_auth(&mut api, &revoked, "revoked-runtime-secret");
    SqliteWorkspaceStore::open(&api.config.database_path)
        .unwrap()
        .revoke_workspace_runtime_binding(
            TEST_WORKSPACE_ID,
            "revoked-runtime-secret",
            "2026-10-08T00:00:00Z",
        )
        .unwrap();
    let spoofed = RuntimeIdentityMaterial::generate("runtime-test").unwrap();
    let unbound_id = "unverified-runtime-secret".repeat(1000);
    let unbound = RuntimeIdentityMaterial::generate(&unbound_id).unwrap();
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/workers/self/workdir-session/operations");
    let target = format!("{path}?access_token=query-secret");
    let body = "body-secret";
    let apps = [
        build_router(api.clone()),
        workspace_server_router(WorkspaceServerApi::new(
            api.config.clone(),
            api.store.clone(),
        )),
    ];

    for app in apps {
        for (case, verification) in [
            ("expired", "signature_verified"),
            ("issued_in_future", "signature_verified"),
            ("replay", "request_verified"),
            ("invalid_format", "unverified"),
            ("invalid_encoding", "unverified"),
            ("malformed_claims", "unverified"),
            ("invalid_signature", "unverified"),
            ("tampered_signature", "unverified"),
            ("revoked_runtime", "unverified"),
            ("runtime_trust_missing_or_revoked", "unverified"),
            ("audience_mismatch", "signature_verified"),
            ("workspace_mismatch", "signature_verified"),
            ("permission_mismatch", "signature_verified"),
            ("method_mismatch", "signature_verified"),
            ("path_mismatch", "signature_verified"),
            ("body_digest_mismatch", "signature_verified"),
            ("worker_catalog_membership", "request_verified"),
        ] {
            let reason = match case {
                "tampered_signature" => "invalid_signature",
                "revoked_runtime" => "runtime_trust_missing_or_revoked",
                other => other,
            };
            let now = i64::try_from(worker_runtime::auth::unix_now_seconds()).unwrap();
            let signing_identity = match case {
                "invalid_signature" => &spoofed,
                "revoked_runtime" => &revoked,
                "runtime_trust_missing_or_revoked" => &unbound,
                _ => &identity,
            };
            let proof = match reason {
                "invalid_format" => "invalid-proof-secret".repeat(1000),
                "invalid_encoding" => "yoi-runtime-request-v1.!.!".to_string(),
                "malformed_claims" => "yoi-runtime-request-v1.e30.AA".to_string(), // {} claims
                _ => RuntimeRequestSourceSigner::from_identity(signing_identity)
                    .issue(
                        if reason == "audience_mismatch" {
                            "audience-secret"
                        } else {
                            "server-test"
                        },
                        if reason == "workspace_mismatch" {
                            "workspace-secret"
                        } else {
                            TEST_WORKSPACE_ID
                        },
                        (reason == "worker_catalog_membership").then_some("worker-secret"),
                        if reason == "permission_mismatch" {
                            "permission-secret"
                        } else {
                            WORKSPACE_REQUEST_PERMISSION
                        },
                        if reason == "method_mismatch" {
                            "GET"
                        } else {
                            "POST"
                        },
                        if reason == "path_mismatch" {
                            "/path-secret"
                        } else {
                            &target
                        },
                        if reason == "body_digest_mismatch" {
                            b"different-body-secret"
                        } else {
                            body.as_bytes()
                        },
                        match reason {
                            "expired" => i64::MIN,
                            "issued_in_future" => i64::MAX,
                            _ => now,
                        },
                        if matches!(reason, "expired" | "issued_in_future") {
                            0
                        } else {
                            30
                        },
                    )
                    .unwrap(),
            };
            let proof = if case == "tampered_signature" {
                let (signed, signature) = proof.rsplit_once('.').unwrap();
                // Flip a significant signature byte, retaining valid base64url.
                let first = if signature.starts_with('A') { "B" } else { "A" };
                format!("{signed}.{first}{}", &signature[1..])
            } else {
                proof
            };
            if reason == "replay" {
                crate::worker_source::verify_runtime_request_source_proof(
                    &api,
                    &proof,
                    TEST_WORKSPACE_ID,
                    WORKSPACE_REQUEST_PERMISSION,
                    "POST",
                    &target,
                    &worker_runtime::auth::request_body_digest(body.as_bytes()),
                )
                .await
                .unwrap();
            }
            let logs = CapturedAuthLog::default();
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(&target)
                        .header(RUNTIME_REQUEST_SOURCE_PROOF_HEADER, &proof)
                        .header(axum::http::header::AUTHORIZATION, "Bearer bearer-secret")
                        .header(CONTENT_TYPE, "application/json")
                        .body(Body::from(body))
                        .unwrap(),
                )
                .with_subscriber(logs.subscriber())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{reason}");
            let response_body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            assert!(response_body.is_empty(), "{response_body:?}");
            let (event, text) = logs.event();
            assert_eq!(event["reason"], reason);
            assert_eq!(event["proof_verification"], verification);
            assert_eq!(event["status"], 401);
            assert_eq!(event["method"], "POST");
            assert_eq!(event["path"], path);
            assert_eq!(event["workspace_id"], TEST_WORKSPACE_ID);
            assert!(uuid::Uuid::parse_str(event["request_id"].as_str().unwrap()).is_ok());
            if verification == "unverified"
                && matches!(
                    reason,
                    "invalid_format" | "invalid_encoding" | "malformed_claims"
                )
            {
                assert!(event.get("claimed_runtime_id_hash").is_none());
                assert!(event.get("claimed_token_id_hash").is_none());
            } else {
                let claims =
                    worker_runtime::auth::decode_runtime_request_source_claims(&proof).unwrap();
                assert_eq!(
                    event["claimed_runtime_id_hash"],
                    crate::worker_source::diagnostic_id_hash(&claims.iss)
                );
                assert_eq!(
                    event["claimed_token_id_hash"],
                    crate::worker_source::diagnostic_id_hash(&claims.jti)
                );
                if let Some(worker) = &claims.worker_id {
                    assert_eq!(
                        event["claimed_worker_id_hash"],
                        crate::worker_source::diagnostic_id_hash(worker)
                    );
                }
            }
            if matches!(reason, "expired" | "issued_in_future") {
                let bound = if reason == "expired" {
                    i64::MIN
                } else {
                    i64::MAX
                };
                assert_eq!(event["issued_at_unix"], bound);
                assert_eq!(event["expires_at_unix"], bound);
                let verified = event["verified_at_unix"].as_i64().unwrap();
                assert_eq!(
                    event["time_delta_seconds"].as_u64(),
                    Some(bound.abs_diff(verified))
                );
                assert_eq!(event["time_tolerance_seconds"], 0);
            } else {
                assert!(event.get("time_delta_seconds").is_none());
                assert!(event.get("issued_at_unix").is_none());
            }
            for secret in [
                &proof,
                &identity.private_key,
                &spoofed.private_key,
                &unbound_id,
                "access_token",
                "query-secret",
                body,
                "bearer-secret",
                "audience-secret",
                "workspace-secret",
                "permission-secret",
                "path-secret",
                "worker-secret",
                "revoked-runtime-secret",
                &revoked.private_key,
            ] {
                assert!(
                    !text.contains(secret),
                    "auth log leaked request data: {reason}"
                );
            }
            assert!(text.len() < 2200, "unbounded diagnostic: {}", text.len());
        }
    }
}

#[tokio::test]
async fn runtime_proof_setup_failure_logs_no_internal_error_and_bounds_request_metadata() {
    let workspace = tempfile::tempdir().unwrap();
    let mut api = test_api(workspace.path()).await;
    let identity = RuntimeIdentityMaterial::generate("runtime-test").unwrap();
    configure_runtime_request_auth(&mut api, &identity, "runtime-test");
    api.config.backend_base_url = None;
    let proof = RuntimeRequestSourceSigner::from_identity(&identity)
        .issue(
            "server-test",
            TEST_WORKSPACE_ID,
            None,
            WORKSPACE_REQUEST_PERMISSION,
            "POST",
            "/request",
            b"{}",
            90,
            10,
        )
        .unwrap();
    let error = crate::worker_source::verify_runtime_request_source_proof(
        &api,
        &proof,
        "private-workspace-secret",
        WORKSPACE_REQUEST_PERMISSION,
        "POST",
        "/request",
        "digest",
    )
    .await
    .unwrap_err();
    let logs = CapturedAuthLog::default();
    let long_path = format!("/{}?secret=query-secret", "é\n".repeat(1000));
    let response = tracing::subscriber::with_default(logs.subscriber(), || {
        runtime_request_proof_rejection(
            &"M\n".repeat(1000),
            &long_path,
            &"w\n".repeat(1000),
            &error,
        )
    });
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .is_empty()
    );
    let (event, text) = logs.event();
    assert_eq!(event["reason"], "backend_audience_unavailable");
    assert_eq!(event["proof_verification"], "unverified");
    for (field, limit) in [("method", 32), ("path", 512), ("workspace_id", 128)] {
        let value = event[field].as_str().unwrap();
        assert!(value.len() <= limit);
        assert!(!value.chars().any(char::is_control));
    }
    for secret in [&proof, "private-workspace-secret", "query-secret"] {
        assert!(!text.contains(secret));
    }
}

#[tokio::test]
async fn repository_runtime_proof_401_keeps_generic_envelope_on_both_routers() {
    let workspace = tempfile::tempdir().unwrap();
    let mut api = test_api(workspace.path()).await;
    let identity = RuntimeIdentityMaterial::generate("runtime-test").unwrap();
    configure_runtime_request_auth(&mut api, &identity, "runtime-test");
    let path = format!("/api/w/{TEST_WORKSPACE_ID}/repositories");
    for app in [
        build_router(api.clone()),
        workspace_server_router(WorkspaceServerApi::new(
            api.config.clone(),
            api.store.clone(),
        )),
    ] {
        for issued_at in [i64::MIN, i64::MAX] {
            let proof = RuntimeRequestSourceSigner::from_identity(&identity)
                .issue(
                    "server-test",
                    TEST_WORKSPACE_ID,
                    None,
                    WORKSPACE_REQUEST_PERMISSION,
                    "GET",
                    &path,
                    b"",
                    issued_at,
                    0,
                )
                .unwrap();
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri(&path)
                        .header(RUNTIME_REQUEST_SOURCE_PROOF_HEADER, &proof)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let actual: Value = serde_json::from_slice(&body).unwrap();
            let expected = serde_json::to_value(server_api::RepositoryApiError::new(
                401,
                "Unauthorized",
                "invalid runtime request proof",
                Vec::new(),
            ))
            .unwrap();
            assert_eq!(
                actual, expected,
                "auth internals must not expand the external envelope"
            );
        }
    }
}
