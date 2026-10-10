use super::*;
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use tower::ServiceExt;
use worker_runtime::{
    Runtime,
    auth::RuntimeIdentityMaterial,
    http_server::{WorkspaceRuntimeHttpAuth, runtime_http_router_with_workspace_auth},
    workspace_issuer::{
        InMemoryWorkspaceClaimReplayProtection, WorkspaceCapabilityVerifier,
        WorkspaceIssuerTrustRecord, WorkspaceIssuerTrustState, issue_workspace_capability_token,
    },
};

#[tokio::test]
async fn restore_capabilities_authorize_runtime_routes_without_relaxing_request_binding() {
    let workspace_identity = RuntimeIdentityMaterial::generate("workspace-key").unwrap();
    let now = Utc::now().timestamp();
    let workspace_public_key_fingerprint =
        crate::workspace_signing_identity::public_key_fingerprint(&workspace_identity.public_key)
            .unwrap();
    let public_key =
        worker_runtime::auth::decode_public_key(&workspace_identity.public_key).unwrap();
    let verifier = WorkspaceCapabilityVerifier::new(
        vec![WorkspaceIssuerTrustRecord {
            workspace_id: "workspace-test".into(),
            backend_url: "https://backend.test".into(),
            key_id: "workspace-key".into(),
            algorithm: "ed25519".into(),
            public_key: workspace_identity.public_key.clone(),
            public_key_fingerprint: format!(
                "sha256:{}",
                Sha256::digest(public_key)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            ),
            trust_id: "trust-restore-test".to_string(),
            state: WorkspaceIssuerTrustState::Active,
            registered_at_unix: now,
            updated_at_unix: now,
        }],
        Arc::new(InMemoryWorkspaceClaimReplayProtection::default()),
    )
    .unwrap();
    // No Worker execution is needed: a typed worker_not_found proves the request passed real auth.
    let app = runtime_http_router_with_workspace_auth(
        Runtime::new_memory(),
        None,
        WorkspaceRuntimeHttpAuth {
            verifier,
            runtime_id: "runtime-test".into(),
        },
    );
    let worker_id = EmbeddedWorkerId::from_legacy_u64(1).to_string();
    let body = serde_json::to_vec(&runtime_api::WorkerRestoreCoordinationRequest {
        expected_observation_token: "observed-runtime-state".into(),
        request_id: "restore-request".into(),
        preparation: None,
    })
    .unwrap();

    for suffix in [
        "restore",
        "restore/coordinate",
        "restore/coordinate?probe=1",
    ] {
        let path = format!("/v1/workers/{worker_id}/{suffix}");
        for case in [
            "authorized",
            "wrong-operation",
            "wrong-worker",
            "wrong-body",
            "wrong-key-fingerprint",
        ] {
            let claims = WorkspaceCapabilityClaims {
                issuer: "https://backend.test".into(),
                issuer_workspace_id: "workspace-test".into(),
                issuer_key_id: "workspace-key".into(),
                issuer_public_key_fingerprint: if case == "wrong-key-fingerprint" {
                    format!(
                        "sha256:{}",
                        Sha256::digest(b"another-workspace-key")
                            .iter()
                            .map(|byte| format!("{byte:02x}"))
                            .collect::<String>()
                    )
                } else {
                    workspace_public_key_fingerprint.clone()
                },
                runtime_id: "runtime-test".into(),
                worker_id: if case == "wrong-worker" {
                    Some(EmbeddedWorkerId::from_legacy_u64(2).to_string())
                } else {
                    worker_id_from_remote_path(&path)
                },
                operation: if case == "wrong-operation" {
                    "runtime:read"
                } else {
                    workspace_runtime_operation("POST", &path)
                }
                .into(),
                method: "POST".into(),
                path_and_query: path.clone(),
                body_digest: workspace_request_body_digest(&body),
                iat: now,
                exp: now + 60,
                jti: uuid::Uuid::now_v7().to_string(),
            };
            let token = issue_workspace_capability_token(
                &workspace_identity.signing_key().unwrap(),
                &claims,
            )
            .unwrap();
            let mut sent_body = body.clone();
            if case == "wrong-body" {
                sent_body.push(b' ');
            }
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(&path)
                        .header("authorization", format!("Bearer {token}"))
                        .header("content-type", "application/json")
                        .body(Body::from(sent_body))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let error: RuntimeHttpErrorResponse = serde_json::from_slice(&bytes).unwrap();
            if case == "authorized" {
                assert_eq!(
                    status,
                    StatusCode::NOT_FOUND,
                    "{path}: {}",
                    error.error.message
                );
                assert_eq!(error.error.code, "worker_not_found", "{path}");
            } else {
                assert_eq!(
                    status,
                    StatusCode::UNAUTHORIZED,
                    "{path} {case}: {}",
                    error.error.message
                );
                assert_eq!(error.error.code, "unauthorized");
                let expected = match case {
                    "wrong-operation" => Some("does not authorize this operation"),
                    "wrong-worker" => Some("targets another Worker"),
                    "wrong-body" => Some("does not bind this request body"),
                    "wrong-key-fingerprint" => None,
                    _ => unreachable!(),
                };
                if let Some(expected) = expected {
                    assert!(
                        error.error.message.contains(expected),
                        "{path} {case}: {}",
                        error.error.message
                    );
                }
            }
        }
    }
}
