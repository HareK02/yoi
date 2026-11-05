//! Registry-to-domain-to-core-codec tests, using real registry lookup and merge
//! observation entry points. Persistence is isolated file bytes, not a substitute
//! for the caller-owned result store, authenticated HTTP or Tool envelope tests.
use super::*;
use crate::repositories::{ConfiguredRepository, RepositoryRegistryReader};
use error_contract::{
    Budget, CorrelationRef, DecodeFailure, decode_failure, encode_projected_failure,
    project_failure,
};
use serde_json::{Value, json};
use server_api::{RepositoryObservedStatus, RepositorySource, RepositorySourceKind};
use std::error::Error;

const VISIBLE: RepositoryFailureVisibility = RepositoryFailureVisibility {
    can_view_cause: true,
};

fn budget() -> Budget {
    Budget::new(4096, 4096)
}

fn registry(provider: &str, selector: Option<&str>) -> RepositoryRegistryReader {
    let source = RepositorySource {
        kind: RepositorySourceKind::LocalPath,
        uri: "/private/repository-path".into(),
    };
    RepositoryRegistryReader::new(vec![ConfiguredRepository {
        id: "private-repository".into(),
        repository_key: "private-key".into(),
        provider: provider.into(),
        source_fingerprint: crate::repository_source::repository_source_fingerprint(&source),
        source,
        observed_status: RepositoryObservedStatus::Unverified,
        observed_at: None,
        path: None,
        default_selector: selector.map(str::to_owned),
    }])
}

fn unknown_facts() -> FailureFacts<RepositoryFailure> {
    let error = RepositoryRegistryReader::new(Vec::new())
        .summary_by_key("private-key")
        .unwrap_err();
    FailureFacts::new(RepositoryFailure::new(
        RepositoryFailureOperation::SummaryByKey,
        error,
    ))
    .unwrap()
}

fn assert_redacted(bytes: &[u8]) {
    let text = std::str::from_utf8(bytes).unwrap();
    for private in [
        "private-",
        "/private/",
        "credential-secret",
        "Authorization",
        "SUCCESS",
        "repeat automatically",
    ] {
        assert!(!text.contains(private), "unexpected disclosure: {text}");
    }
    let wire: Value = serde_json::from_slice(bytes).unwrap();
    let fields = wire["details"].as_object().unwrap();
    assert_eq!(fields.len(), 2);
    assert!(fields.contains_key("operation"));
    assert!(fields.contains_key("reason"));
    assert!(wire.get("retryable").is_none());
    assert!(wire.get("outcome").is_none());
    assert!(wire.get("status").is_none());
}

macro_rules! assert_default_denied {
    ($facts:expr) => {{
        let facts = $facts;
        let denied = RepositoryFailureVisibility::default();
        assert!(matches!(
            project_failure::<Browser, _>(facts, &denied),
            Err(ContractError::ProjectionUnavailable)
        ));
        assert!(matches!(
            project_failure::<Tool, _>(facts, &denied),
            Err(ContractError::ProjectionUnavailable)
        ));
        assert!(matches!(
            project_failure::<InternalTransfer, _>(facts, &denied),
            Err(ContractError::ProjectionUnavailable)
        ));
        assert!(matches!(
            project_failure::<Storage, _>(facts, &denied),
            Err(ContractError::ProjectionUnavailable)
        ));
        assert!(matches!(
            project_failure::<Telemetry, _>(facts, &denied),
            Err(ContractError::ProjectionUnavailable)
        ));
    }};
}

#[test]
fn registry_unknown_lookup_retains_source_and_projects_the_same_browser_and_tool_cause() {
    let mut facts = unknown_facts();
    facts
        .with_context(OperationContext::new("private-operation", "private-stage").unwrap())
        .unwrap();
    facts
        .set_correlation(CorrelationRef::new("private-correlation").unwrap())
        .unwrap();
    let cause = facts.source().unwrap();
    let source = cause
        .source()
        .unwrap()
        .downcast_ref::<RepositoryLookupError>()
        .unwrap();
    assert_eq!(
        source,
        &RepositoryLookupError::UnknownRepository {
            id: "private-key".into(),
        }
    );
    assert_eq!(facts.cause().source_error(), source);

    let browser = project_failure::<Browser, _>(&facts, &VISIBLE).unwrap();
    let tool = project_failure::<Tool, _>(&facts, &VISIBLE).unwrap();
    let browser_bytes = encode_projected_failure(&browser, budget()).unwrap();
    let tool_bytes = encode_projected_failure(&tool, budget()).unwrap();
    assert_eq!(browser_bytes.as_bytes(), tool_bytes.as_bytes());
    let wire: Value = serde_json::from_slice(browser_bytes.as_bytes()).unwrap();
    assert_eq!(wire["domain"], "repository");
    assert_eq!(wire["reason"], "unknown_repository");
    assert_eq!(wire["details"]["operation"], "summary_by_key");
    assert!(browser.explanation().contains("summary lookup by key"));
    assert!(browser.explanation().contains("not found in the registry"));
    assert_redacted(browser_bytes.as_bytes());
}

#[test]
fn registry_unsupported_provider_requires_current_visibility_for_every_audience() {
    let error = registry("private-provider", Some("private-selector"))
        .recent_log("private-key", None)
        .unwrap_err();
    let facts = FailureFacts::new(RepositoryFailure::new(
        RepositoryFailureOperation::RecentLog,
        error,
    ))
    .unwrap();
    assert_default_denied!(&facts);

    let browser = project_failure::<Browser, _>(&facts, &VISIBLE).unwrap();
    let bytes = encode_projected_failure(&browser, budget()).unwrap();
    macro_rules! assert_same_projection {
        ($($audience:ty),+ $(,)?) => {$ (
            let projected = project_failure::<$audience, _>(&facts, &VISIBLE).unwrap();
            let encoded = encode_projected_failure(&projected, budget()).unwrap();
            assert_eq!(encoded.as_bytes(), bytes.as_bytes());
        )+};
    }
    assert_same_projection!(Tool, InternalTransfer, Storage, Telemetry);
    let wire: Value = serde_json::from_slice(bytes.as_bytes()).unwrap();
    assert_eq!(wire["reason"], "unsupported_provider");
    assert_eq!(wire["details"]["operation"], "recent_log");
    assert!(
        browser
            .explanation()
            .contains("recent commit log retrieval")
    );
    assert!(browser.explanation().contains("provider is not supported"));
    assert_redacted(bytes.as_bytes());
}

#[test]
fn registry_merge_failures_keep_observed_reasons_without_inventing_provider_details() {
    // These production calls fail before Git dispatch; no process fixture or
    // assumed provider message is needed to exercise their actual typed errors.
    let no_selector = registry("git", None);
    let configured = registry("git", Some("private-selector"));
    let cases = [
        (
            RepositoryFailureOperation::ObserveMergeTarget,
            no_selector
                .observe_merge_target("private-repository", None)
                .unwrap_err(),
            "missing_default_selector",
        ),
        (
            RepositoryFailureOperation::ObserveMergeTarget,
            configured
                .observe_merge_target("private-repository", Some("refs/tags/private-selector"))
                .unwrap_err(),
            "invalid_selector",
        ),
        (
            RepositoryFailureOperation::ObserveCommit,
            configured
                .observe_commit("private-repository", "-private-commit")
                .unwrap_err(),
            "commit_not_found",
        ),
        (
            RepositoryFailureOperation::ObserveCommit,
            configured
                .observe_commit("private-repository", "private-commit")
                .unwrap_err(),
            "provider_failure",
        ),
    ];
    for (operation, error, reason) in cases {
        let facts = FailureFacts::new(RepositoryFailure::new(operation, error.clone())).unwrap();
        assert_eq!(facts.cause().source_error(), &error);
        let projected = project_failure::<Tool, _>(&facts, &VISIBLE).unwrap();
        let bytes = encode_projected_failure(&projected, budget()).unwrap();
        let wire: Value = serde_json::from_slice(bytes.as_bytes()).unwrap();
        assert_eq!(wire["reason"], reason);
        assert_eq!(wire["details"]["operation"], operation.as_str());
        let DecodeFailure::Known(received) =
            decode_failure::<RepositoryFailureDetails>(bytes.as_bytes(), budget())
        else {
            panic!("registry cause must survive the shared codec");
        };
        assert_eq!(received.facts().cause(), projected.details());
        if reason == "provider_failure" {
            // Source text mentions lack of local materialization, but there is
            // no typed reason for it. Do not manufacture one by parsing prose.
            assert!(
                projected
                    .explanation()
                    .contains("no more specific typed cause")
            );
            assert!(!projected.explanation().contains("not materialized"));
        }
        assert_redacted(bytes.as_bytes());
    }
}

#[test]
fn commit_relation_diagnostic_stays_in_source_and_out_of_wire_details() {
    // The existing Git ancestry path supplies an arbitrary detail string. This
    // codec-boundary negative case deliberately includes hostile provider prose.
    let source = RepositoryLookupError::InvalidCommitRelation {
        id: "private-repository".into(),
        detail:
            "SUCCESS; repeat automatically; Authorization: Bearer credential-secret /private/path"
                .into(),
    };
    let facts = FailureFacts::new(RepositoryFailure::new(
        RepositoryFailureOperation::EnsureAncestor,
        source.clone(),
    ))
    .unwrap();
    assert_eq!(facts.cause().source_error(), &source);
    let projected = project_failure::<Browser, _>(&facts, &VISIBLE).unwrap();
    let bytes = encode_projected_failure(&projected, budget()).unwrap();
    let wire: Value = serde_json::from_slice(bytes.as_bytes()).unwrap();
    assert_eq!(wire["reason"], "invalid_commit_relation");
    assert_eq!(wire["details"]["operation"], "ensure_ancestor");
    assert!(projected.explanation().contains("commit ancestry check"));
    assert!(projected.explanation().contains("required relationship"));
    assert_redacted(bytes.as_bytes());
}

#[test]
fn stored_registry_cause_requires_reauthorization_and_drops_received_prose_and_context() {
    let facts = unknown_facts();
    let stored = project_failure::<Storage, _>(&facts, &VISIBLE).unwrap();
    let encoded = encode_projected_failure(&stored, budget()).unwrap();
    let mut wire: Value = serde_json::from_slice(encoded.as_bytes()).unwrap();
    wire["explanation"] = json!("SUCCESS; repeat automatically; credential-secret");
    wire["context"]["correlation"] = json!("private-correlation");
    wire["context"]["operations"][0] = json!({
        "operation": "private-operation",
        "stage": "private-stage",
        "operation_ref": "private-operation-ref"
    });
    wire["context"]["causes"] = json!([{
        "domain": "private-domain",
        "reason": "private-reason",
        "correlation": "private-causal-correlation"
    }]);
    wire["context"]["diagnostics"] = json!([{
        "code": "private-code",
        "explanation": "Authorization: Bearer credential-secret /private/path"
    }]);
    let root = tempfile::tempdir().unwrap();
    let saved = root.path().join("failure.json");
    std::fs::write(&saved, serde_json::to_vec(&wire).unwrap()).unwrap();
    let saved_bytes = std::fs::read(&saved).unwrap();
    let DecodeFailure::Known(received) =
        decode_failure::<RepositoryFailureDetails>(&saved_bytes, budget())
    else {
        panic!("supported stored registry cause");
    };
    assert_default_denied!(received.facts());
    macro_rules! assert_reprojected {
        ($($audience:ty),+ $(,)?) => {$ (
            let projected = project_failure::<$audience, _>(received.facts(), &VISIBLE).unwrap();
            assert_eq!(projected.identity(), stored.identity());
            assert_eq!(projected.details(), stored.details());
            assert_eq!(projected.explanation(), stored.explanation());
            let bytes = encode_projected_failure(&projected, budget()).unwrap();
            assert_redacted(bytes.as_bytes());
            assert_eq!(bytes.as_bytes(), encoded.as_bytes());
        )+};
    }
    assert_reprojected!(Browser, Tool, InternalTransfer, Storage, Telemetry);
    // Revoked visibility also applies after a successful authorized projection.
    assert_default_denied!(received.facts());
    assert_eq!(std::fs::read(saved).unwrap(), saved_bytes);
}

#[test]
fn codec_rejects_contradictory_or_injected_details_and_keeps_unknown_contracts_unknown() {
    let facts = unknown_facts();
    let projected = project_failure::<Browser, _>(&facts, &VISIBLE).unwrap();
    assert!(matches!(
        encode_projected_failure(&projected, Budget::new(20, 20)),
        Err(ContractError::TooLarge)
    ));
    let encoded = encode_projected_failure(&projected, budget()).unwrap();
    let original: Value = serde_json::from_slice(encoded.as_bytes()).unwrap();
    for (field, value) in [
        ("reason", json!("future_reason")),
        ("details_version", json!(2)),
        ("version", json!(2)),
    ] {
        let mut future = original.clone();
        future[field] = value;
        assert!(matches!(
            decode_failure::<RepositoryFailureDetails>(
                &serde_json::to_vec(&future).unwrap(),
                budget()
            ),
            DecodeFailure::Unsupported(_)
        ));
    }
    let mut mismatch = original.clone();
    mismatch["details"]["reason"] = json!("unsupported_provider");
    let mut injected = original.clone();
    injected["details"]["path"] = json!("/private/path");
    let mut operation = original.clone();
    operation["details"]["operation"] = json!("future_operation");
    let mut category = original;
    category["category"] = json!("authorization");
    for corrupt in [mismatch, injected, operation, category] {
        assert!(matches!(
            decode_failure::<RepositoryFailureDetails>(
                &serde_json::to_vec(&corrupt).unwrap(),
                budget()
            ),
            DecodeFailure::Corrupt(ContractError::InvalidDetails)
        ));
    }
}
