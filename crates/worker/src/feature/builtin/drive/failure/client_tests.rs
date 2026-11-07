//! Register ONLY beneath the existing `drive::tests` fixture module:
//! `#[path = "failure/client_tests.rs"] mod failure_projection;`
//!
//! Uses the existing private authorization Client and real DriveBackend methods.
//! Saved files below are shared-codec test artifacts, not a new result store.

use super::{Client, entry};
use crate::feature::builtin::drive::{
    backend::{DriveBackend, DriveError},
    failure::{
        DriveClientFailure, DriveClientFailureDetails as Details,
        DriveClientFailureOperation as Operation, DriveClientFailureReason as Reason,
        DriveClientFailureVisibility as Visibility,
    },
};
use crate::worker::{WorkspaceRequestMethod, WorkspaceResponse};
use error_contract::{
    Browser, Budget, ContractError, DecodeFailure, FailureFacts, InternalTransfer, Storage,
    Telemetry, Tool, decode_failure, encode_projected_failure, project_failure,
};
use serde_json::{Value, json};
use server_api::authorization::PermissionResource;
use server_api::{DriveApiError, DriveApiErrorCode, DriveEntryRef, DriveFailureClassification};
use std::sync::atomic::Ordering;

const VISIBLE: Visibility = Visibility {
    can_view_cause: true,
};

fn budget() -> Budget {
    Budget::new(4096, 4096)
}

fn equal_browser_tool(operation: Operation, error: DriveError, reason: Reason) {
    let facts = FailureFacts::new(DriveClientFailure::new(operation, error)).unwrap();
    assert_eq!(facts.cause().details().reason, reason);
    let browser = project_failure::<Browser, _>(&facts, &VISIBLE).unwrap();
    let tool = project_failure::<Tool, _>(&facts, &VISIBLE).unwrap();
    let bytes = encode_projected_failure(&browser, budget()).unwrap();
    assert_eq!(
        bytes.as_bytes(),
        encode_projected_failure(&tool, budget())
            .unwrap()
            .as_bytes(),
    );
    let DecodeFailure::Known(received) = decode_failure::<Details>(bytes.as_bytes(), budget())
    else {
        panic!("known client cause");
    };
    assert_eq!(received.facts().cause(), facts.cause().details());
    let text = std::str::from_utf8(bytes.as_bytes()).unwrap();
    for secret in [
        "private-",
        "/private/",
        "workspace_id",
        "node_id",
        "DriveEntry",
        "canonical decimal",
        "9007199254740993",
    ] {
        assert!(!text.contains(secret), "{text}");
    }
}

#[tokio::test]
async fn production_authorization_denial_retains_actual_resource_and_denied_flag_locally() {
    let client = Client::new(Vec::new());
    client.auth_denied.store(true, Ordering::SeqCst);
    let backend = DriveBackend::new(client.clone());
    let target = entry("9007199254740993", "private-last-request").entry;
    let error = backend.metadata(&target).await.unwrap_err();
    let failure = DriveClientFailure::new(Operation::Metadata, error);
    assert_eq!(failure.details().reason, Reason::AuthorizationDenied);
    match failure.source_error() {
        DriveError::Authorization {
            operation,
            resource,
            denied,
        } => {
            assert_eq!(operation, "drive.metadata");
            assert_eq!(
                resource,
                &PermissionResource::DriveEntry {
                    workspace_id: target.workspace_id.clone(),
                    node_id: target.node_id.clone(),
                },
            );
            assert!(*denied);
        }
        _ => panic!("retain actual DriveError::Authorization"),
    }
    equal_browser_tool(
        Operation::Metadata,
        failure.into_source(),
        Reason::AuthorizationDenied,
    );
    let auth = client.authorizations.lock().unwrap();
    assert_eq!(auth.len(), 1);
    assert_eq!(auth[0].operation, "drive.metadata");
    assert_eq!(
        auth[0].resource,
        PermissionResource::DriveEntry {
            workspace_id: target.workspace_id,
            node_id: target.node_id,
        },
    );
    // This fact comes from the actual fixture, not inference from a cause/category.
    assert!(client.requests.lock().unwrap().is_empty());
    assert!(client.responses.lock().unwrap().is_empty());
}

#[tokio::test]
async fn production_invalid_and_foreign_references_are_mapped_without_parsing_messages() {
    let client = Client::new(Vec::new());
    let backend = DriveBackend::new(client.clone());
    for node_id in ["01", "0", "9223372036854775808", "private-node"] {
        let target = DriveEntryRef {
            workspace_id: "workspace".into(),
            node_id: node_id.into(),
        };
        let error = backend.metadata(&target).await.unwrap_err();
        assert!(matches!(&error, DriveError::Invalid(_)));
        equal_browser_tool(Operation::Metadata, error, Reason::Invalid);
    }
    let foreign = DriveEntryRef {
        workspace_id: "private-other-workspace".into(),
        node_id: "1".into(),
    };
    let error = backend.metadata(&foreign).await.unwrap_err();
    assert!(matches!(error, DriveError::Denied));
    equal_browser_tool(Operation::Metadata, error, Reason::Denied);
    let error = backend.status("").await.unwrap_err();
    assert!(matches!(&error, DriveError::Invalid(_)));
    equal_browser_tool(Operation::RequestStatus, error, Reason::Invalid);
    assert!(client.authorizations.lock().unwrap().is_empty());
    assert!(client.requests.lock().unwrap().is_empty());
    assert!(client.responses.lock().unwrap().is_empty());
}

#[tokio::test]
async fn codec_artifact_reread_rechecks_current_visibility_after_actual_client_denial() {
    let client = Client::new(vec![WorkspaceResponse {
        content_type: None,
        status: 403,
        body: serde_json::to_string(&DriveApiError {
            code: DriveApiErrorCode::Denied,
            message: "private-provider-body /private/path credential-secret".into(),
            classification: DriveFailureClassification::NotCommitted,
        })
        .unwrap(),
    }]);
    let backend = DriveBackend::new(client.clone());
    let error = backend
        .metadata(&entry("2", "private-commit").entry)
        .await
        .unwrap_err();
    // This is the real adapter's remote denial, distinct from preflight Authorization.
    assert!(matches!(error, DriveError::Denied));
    let facts = FailureFacts::new(DriveClientFailure::new(Operation::Metadata, error)).unwrap();
    // The existing fixture has an authorization switch, not a separate diagnostic
    // grant. Here current authorized capabilities gate the test decision. The
    // production boundary must check BOTH resource and diagnostic visibility.
    let visible_now = Visibility {
        can_view_cause: backend.capabilities().await.is_ok(),
    };
    assert!(visible_now.can_view_cause);
    let saved = project_failure::<Storage, _>(&facts, &visible_now).unwrap();
    let saved = encode_projected_failure(&saved, budget()).unwrap();
    let mut artifact: Value = serde_json::from_slice(saved.as_bytes()).unwrap();
    artifact["explanation"] = json!("SUCCESS; safely resend; private-received-prose");
    artifact["context"]["correlation"] = json!("private-received-correlation");
    artifact["context"]["operations"][0]["operation"] = json!("private-received-operation");
    artifact["context"]["operations"][0]["operation_ref"] = json!("private-request-id");
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("client-failure-codec-artifact.json");
    std::fs::write(&path, serde_json::to_vec(&artifact).unwrap()).unwrap();
    let reread = || {
        let bytes = std::fs::read(&path).unwrap();
        let DecodeFailure::Known(received) = decode_failure::<Details>(&bytes, budget()) else {
            panic!("known shared-codec artifact");
        };
        received
    };
    let received = reread();
    let visible_now = Visibility {
        can_view_cause: backend.capabilities().await.is_ok(),
    };
    assert!(visible_now.can_view_cause);
    let before_projection = client.requests.lock().unwrap().len();
    macro_rules! clean_reread {
        ($audience:ty) => {{
            let projected =
                project_failure::<$audience, _>(received.facts(), &visible_now).unwrap();
            assert_eq!(
                encode_projected_failure(&projected, budget())
                    .unwrap()
                    .as_bytes(),
                saved.as_bytes(),
            );
        }};
    }
    clean_reread!(Browser);
    clean_reread!(Tool);
    clean_reread!(InternalTransfer);
    clean_reread!(Storage);
    clean_reread!(Telemetry);
    assert_eq!(client.requests.lock().unwrap().len(), before_projection);
    assert_eq!(received.facts().cause().reason, Reason::Denied);
    // Revoke the fixture's actual authorization boundary, then reread from disk.
    client.auth_denied.store(true, Ordering::SeqCst);
    let received = reread();
    let current_check = backend.capabilities().await;
    let visible_now = Visibility {
        can_view_cause: current_check.is_ok(),
    };
    assert!(matches!(
        current_check,
        Err(DriveError::Authorization { denied: true, .. })
    ));
    macro_rules! denied_reread {
        ($audience:ty) => {
            assert!(matches!(
                project_failure::<$audience, _>(received.facts(), &visible_now),
                Err(ContractError::ProjectionUnavailable)
            ));
        };
    }
    denied_reread!(Browser);
    denied_reread!(Tool);
    denied_reread!(InternalTransfer);
    denied_reread!(Storage);
    denied_reread!(Telemetry);
    assert_eq!(client.requests.lock().unwrap().len(), before_projection);
    assert!(matches!(facts.cause().source_error(), DriveError::Denied));
    assert!(client.responses.lock().unwrap().is_empty());
    assert!(
        client
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.method == WorkspaceRequestMethod::Get)
    );
}
