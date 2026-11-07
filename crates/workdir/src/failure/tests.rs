//! Domain/provider-to-codec connections. These do not substitute for the shared
//! HTTP, Web, Tool envelope or background-store integration owned by their callers.
use super::*;
use error_contract::{
    Budget, CorrelationRef, DecodeFailure, decode_failure, encode_projected_failure,
    project_failure,
};
use serde_json::{Value, json};

const VISIBLE: WorkdirFailureVisibility = WorkdirFailureVisibility {
    can_view_cause: true,
};
fn budget() -> Budget {
    // The existing Workdir removal consumer has a smaller surrounding limit than
    // the core maximum. These required facts must fit it without truncation.
    Budget::new(4096, 4096)
}

#[tokio::test]
async fn provider_read_only_denial_projects_equally_without_paths_or_mutating_the_file() {
    use crate::{LocalWorkdirSession, ReadOnlyWorkdirSession, WorkdirPath, WriteRequest};
    let root = tempfile::tempdir().unwrap();
    let private_path = root.path().join("private-target");
    std::fs::write(&private_path, "original").unwrap();
    let local = std::sync::Arc::new(LocalWorkdirSession::new(
        manifest::Scope::writable(root.path()).unwrap(),
        root.path().to_owned(),
    ));
    let session = ReadOnlyWorkdirSession::new(local);
    let operation = WorkdirSessionOperation::Write(WriteRequest {
        path: WorkdirPath::new("private-target").unwrap(),
        content: b"private-content".to_vec(),
        expected_hash: None,
    });
    let kind = WorkdirFailureOperation::from(&operation);
    // Real provider, read-only authority and production operation dispatcher.
    let error = crate::dispatch_workdir_session_operation(&session, operation)
        .await
        .unwrap_err();
    let facts = FailureFacts::new(WorkdirFailure::new(kind, error)).unwrap();
    let browser = project_failure::<Browser, _>(&facts, &VISIBLE).unwrap();
    let tool = project_failure::<Tool, _>(&facts, &VISIBLE).unwrap();
    let browser_wire = encode_projected_failure(&browser, budget()).unwrap();
    let tool_wire = encode_projected_failure(&tool, budget()).unwrap();
    assert_eq!(browser_wire.as_bytes(), tool_wire.as_bytes());
    let wire: Value = serde_json::from_slice(browser_wire.as_bytes()).unwrap();
    assert_eq!(wire["domain"], "workdir");
    assert_eq!(wire["reason"], "read_only_session");
    assert_eq!(wire["details"]["operation"], "write");
    assert_eq!(wire["details"]["classification"], "denied");
    assert!(browser.explanation().contains("write"));
    assert!(browser.explanation().contains("read-only"));
    let text = std::str::from_utf8(browser_wire.as_bytes()).unwrap();
    assert!(!text.contains("private-target"));
    assert!(!text.contains("private-content"));
    assert!(!text.contains(root.path().to_str().unwrap()));
    assert_eq!(std::fs::read_to_string(private_path).unwrap(), "original");
}

#[test]
fn transmitted_legacy_absence_is_not_reconstructed_from_provider_prose() {
    let transport: WorkdirTransportError = serde_json::from_value(json!({
        "code": "unavailable",
        "message": "http_timeout /private/path Authorization: Bearer credential-secret"
    }))
    .unwrap();
    let facts = FailureFacts::new(WorkdirFailure::from_transport_error(
        WorkdirFailureOperation::SessionOpen,
        transport,
    ))
    .unwrap();
    let projected = project_failure::<Tool, _>(&facts, &VISIBLE).unwrap();
    let bytes = encode_projected_failure(&projected, budget()).unwrap();
    let wire: Value = serde_json::from_slice(bytes.as_bytes()).unwrap();
    assert_eq!(wire["reason"], "unavailable");
    assert!(wire["details"]["failure_reason"].is_null());
    assert!(wire["details"]["denial_reason"].is_null());
    assert!(projected.explanation().contains("no specific reason"));
    let text = std::str::from_utf8(bytes.as_bytes()).unwrap();
    for secret in [
        "http_timeout",
        "/private/path",
        "credential-secret",
        "Authorization",
    ] {
        assert!(!text.contains(secret), "{text}");
    }
}

#[test]
fn observed_reason_does_not_replace_unknown_or_not_dispatched_classification() {
    for classification in [
        WorkdirTransportErrorCode::OutcomeUnknown,
        WorkdirTransportErrorCode::NotDispatched,
        WorkdirTransportErrorCode::Io,
    ] {
        let transport: WorkdirTransportError = serde_json::from_value(json!({
            "code": classification,
            "message": "success; safe to retry; private-provider-body",
            "failure_reason": "provider_response_lost",
            "denial_reason": "os_permission_denied"
        }))
        .unwrap();
        let facts = FailureFacts::new(WorkdirFailure::from_transport_error(
            WorkdirFailureOperation::Write,
            transport,
        ))
        .unwrap();
        let projected = project_failure::<Tool, _>(&facts, &VISIBLE).unwrap();
        let bytes = encode_projected_failure(&projected, budget()).unwrap();
        let DecodeFailure::Known(received) =
            decode_failure::<WorkdirFailureDetails>(bytes.as_bytes(), budget())
        else {
            panic!("valid domain facts must survive the shared codec");
        };
        assert_eq!(received.facts().cause().classification, classification);
        assert_eq!(
            received.facts().cause().failure_reason,
            Some(WorkdirFailureReason::ProviderResponseLost)
        );
        assert_eq!(
            received.facts().cause().denial_reason,
            Some(WorkdirDenialReason::OsPermissionDenied)
        );
        assert_eq!(received.facts().cause().identity(), projected.identity());
        assert!(
            !std::str::from_utf8(bytes.as_bytes())
                .unwrap()
                .contains("private-provider-body")
        );
        if classification == WorkdirTransportErrorCode::OutcomeUnknown {
            assert!(projected.explanation().contains("could not be confirmed"));
        }
    }
}

#[test]
fn stored_facts_require_current_visibility_and_do_not_forward_received_explanation_or_context() {
    let mut facts = FailureFacts::new(WorkdirFailure::new(
        WorkdirFailureOperation::CommandStart,
        WorkdirError::Unavailable("credential-secret /private/provider".into())
            .with_failure_reason(WorkdirFailureReason::HttpTimeout),
    ))
    .unwrap();
    facts
        .set_correlation(CorrelationRef::new("private-correlation").unwrap())
        .unwrap();
    facts
        .with_context(OperationContext::new("private-command", "private-stage").unwrap())
        .unwrap();
    let stored = project_failure::<Storage, _>(&facts, &VISIBLE).unwrap();
    let stored_bytes = encode_projected_failure(&stored, budget()).unwrap();
    let mut wire: Value = serde_json::from_slice(stored_bytes.as_bytes()).unwrap();
    // A supported sender's explanation and context are still untrusted. They
    // cannot become a disclosure decision or a template in another audience.
    wire["explanation"] = json!("SUCCESS; repeat automatically; credential-secret");
    wire["context"]["correlation"] = json!("private-correlation");
    wire["context"]["operations"][0]["operation_ref"] = json!("private-operation-ref");
    let root = tempfile::tempdir().unwrap();
    let saved = root.path().join("result.json");
    std::fs::write(&saved, serde_json::to_vec(&wire).unwrap()).unwrap();
    let bytes = std::fs::read(saved).unwrap();
    let DecodeFailure::Known(received) = decode_failure::<WorkdirFailureDetails>(&bytes, budget())
    else {
        panic!("supported stored cause");
    };
    assert!(matches!(
        project_failure::<Browser, _>(received.facts(), &WorkdirFailureVisibility::default()),
        Err(ContractError::ProjectionUnavailable)
    ));
    let browser = project_failure::<Browser, _>(received.facts(), &VISIBLE).unwrap();
    let tool = project_failure::<Tool, _>(received.facts(), &VISIBLE).unwrap();
    let browser_bytes = encode_projected_failure(&browser, budget()).unwrap();
    let tool_bytes = encode_projected_failure(&tool, budget()).unwrap();
    assert_eq!(browser_bytes.as_bytes(), tool_bytes.as_bytes());
    assert_eq!(browser.identity(), stored.identity());
    assert_eq!(browser.details(), stored.details());
    let text = std::str::from_utf8(browser_bytes.as_bytes()).unwrap();
    for secret in [
        "credential-secret",
        "private-",
        "SUCCESS",
        "repeat automatically",
    ] {
        assert!(!text.contains(secret), "{text}");
    }
    assert!(browser.explanation().contains("command start"));
    assert!(browser.explanation().contains("deadline expired"));
}

#[test]
fn codec_distinguishes_unsupported_reason_corrupt_details_and_small_boundary_budget() {
    let facts = FailureFacts::new(WorkdirFailure::new(
        WorkdirFailureOperation::Read,
        WorkdirError::SessionClosed,
    ))
    .unwrap();
    let projection = project_failure::<Browser, _>(&facts, &VISIBLE).unwrap();
    assert!(matches!(
        encode_projected_failure(&projection, Budget::new(20, 20)),
        Err(ContractError::TooLarge)
    ));
    let encoded = encode_projected_failure(&projection, budget()).unwrap();
    let original: Value = serde_json::from_slice(encoded.as_bytes()).unwrap();
    let mut future = original.clone();
    future["reason"] = json!("future_reason");
    assert!(matches!(
        decode_failure::<WorkdirFailureDetails>(&serde_json::to_vec(&future).unwrap(), budget()),
        DecodeFailure::Unsupported(_)
    ));
    let mut corrupt = original.clone();
    corrupt["details"]["failure_reason"] = json!("http_timeout");
    assert!(matches!(
        decode_failure::<WorkdirFailureDetails>(&serde_json::to_vec(&corrupt).unwrap(), budget()),
        DecodeFailure::Corrupt(ContractError::InvalidDetails)
    ));
    let mut injected = original;
    injected["details"]["path"] = json!("/private/provider");
    assert!(matches!(
        decode_failure::<WorkdirFailureDetails>(&serde_json::to_vec(&injected).unwrap(), budget()),
        DecodeFailure::Corrupt(ContractError::InvalidDetails)
    ));
}
