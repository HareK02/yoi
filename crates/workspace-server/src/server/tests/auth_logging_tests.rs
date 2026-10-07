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

#[test]
fn runtime_proof_diagnostics_redact_error_payloads() {
    use crate::worker_source::WorkerMutationSourceProofError;

    for (error, reason) in [
        (
            WorkerMutationSourceProofError::Authority("private-database-path".to_string()),
            "authority_unavailable",
        ),
        (
            WorkerMutationSourceProofError::MissingPermission("private-permission".to_string()),
            "missing_permission",
        ),
    ] {
        let logs = CapturedAuthLog::default();
        let writer = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .json()
            .flatten_event(true)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            let response = runtime_request_proof_rejection(
                "POST",
                "/api/w/workspace/workers/self/workdir-session/operations?secret=query-secret",
                "workspace",
                &error,
            );
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        });
        let text = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
        let event: Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(event["reason"], reason);
        assert!(!text.contains("private-"), "auth log leaked error details");
        assert!(
            !text.contains("query-secret"),
            "auth log leaked query values"
        );
    }
}

#[tokio::test]
async fn rejected_runtime_proofs_log_reason_without_request_secrets_on_both_routers() {
    let workspace = tempfile::tempdir().unwrap();
    let mut api = test_api(workspace.path()).await;
    let identity = RuntimeIdentityMaterial::generate("runtime-test").unwrap();
    configure_runtime_request_auth(&mut api, &identity, "runtime-test");
    let signer = RuntimeRequestSourceSigner::from_identity(&identity);
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
        for reason in ["expired", "replay", "invalid"] {
            let now = i64::try_from(worker_runtime::auth::unix_now_seconds()).unwrap();
            let proof = if reason == "invalid" {
                "invalid-proof-secret".to_string()
            } else {
                signer
                    .issue(
                        "server-test",
                        TEST_WORKSPACE_ID,
                        None,
                        WORKSPACE_REQUEST_PERMISSION,
                        "POST",
                        &target,
                        body.as_bytes(),
                        if reason == "expired" { now - 120 } else { now },
                        30,
                    )
                    .unwrap()
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
            let writer = logs.clone();
            let subscriber = tracing_subscriber::fmt()
                .with_max_level(tracing::Level::WARN)
                .without_time()
                .with_ansi(false)
                .json()
                .flatten_event(true)
                .with_writer(move || writer.clone())
                .finish();
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
                .with_subscriber(subscriber)
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            let response_body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            assert!(response_body.is_empty(), "{response_body:?}");

            let text = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
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
            let event = &events[0];
            assert_eq!(event["reason"], reason);
            assert_eq!(event["status"], 401);
            assert_eq!(event["method"], "POST");
            assert_eq!(event["path"], path);
            assert_eq!(event["workspace_id"], TEST_WORKSPACE_ID);
            for secret in [
                &proof,
                "access_token",
                "query-secret",
                body,
                "bearer-secret",
            ] {
                assert!(!text.contains(secret), "auth log leaked request data");
            }
        }
    }
}
