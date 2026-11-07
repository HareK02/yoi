//! Component connections: real config-source boundaries -> local cause -> shared
//! projection/codec and file-backed attachment reread. These do not exercise the
//! owner HTTP authorization, adoption transaction, Tool envelope or receipt store;
//! their integration tests belong to the callers. Runs in the normal Server lib suite.

use super::*;
use config_source::{
    ConfigContentType, ConfigEntry, ConfigTreeChange, ConfigTreeSnapshot, VirtualPath,
};
use error_contract::{
    Budget, CorrelationRef, DecodeFailure, OperationContext,
    compatibility::{StoredFailureRead, read_failure_attachment},
    decode_failure, encode_projected_failure, project_failure,
};
use serde_json::{Value, json};
use std::error::Error as _;

const VISIBLE: ConfigurationFailureVisibility = ConfigurationFailureVisibility {
    can_view_cause: true,
};
const SECRET: &str = "credential-secret";

fn budget() -> Budget {
    Budget::new(4096, 4096)
}

fn snapshot() -> ConfigTreeSnapshot {
    ConfigTreeSnapshot::from_entries([ConfigEntry::new(
        VirtualPath::parse("private-source.dcdl").unwrap(),
        ConfigContentType::Decodal,
        format!("{{ token = \"{SECRET}\"; }}"),
    )
    .unwrap()])
    .unwrap()
}

fn stale_entry_failure() -> (ConfigTreeSnapshot, ConfigurationFailure) {
    let snapshot = snapshot();
    let error = snapshot
        .apply(&[ConfigTreeChange::Update {
            path: VirtualPath::parse("private-source.dcdl").unwrap(),
            expected_digest: "private-stale-digest".into(),
            content: format!("{{ token = \"replacement-{SECRET}\"; }}"),
        }])
        .unwrap_err();
    (
        snapshot,
        ConfigurationFailure::from_tree(ConfigurationFailureOperation::Preview, error),
    )
}

fn browser_tool_wire(facts: &FailureFacts<ConfigurationFailure>) -> Value {
    let browser = project_failure::<Browser, _>(facts, &VISIBLE).unwrap();
    let tool = project_failure::<Tool, _>(facts, &VISIBLE).unwrap();
    let browser_bytes = encode_projected_failure(&browser, budget()).unwrap();
    let tool_bytes = encode_projected_failure(&tool, budget()).unwrap();
    assert_eq!(browser.identity(), tool.identity());
    assert_eq!(browser.details(), tool.details());
    assert_eq!(browser.explanation(), tool.explanation());
    assert_eq!(browser_bytes.as_bytes(), tool_bytes.as_bytes());
    let text = std::str::from_utf8(browser_bytes.as_bytes()).unwrap();
    assert!(!text.contains(SECRET), "{text}");
    assert!(!text.contains("private-"), "{text}");
    let DecodeFailure::Known(received) =
        decode_failure::<ConfigurationFailureDetails>(browser_bytes.as_bytes(), budget())
    else {
        panic!("configuration facts must survive the shared codec");
    };
    assert_eq!(received.facts().cause().identity(), browser.identity());
    assert_eq!(received.facts().cause(), browser.details());
    serde_json::from_slice(browser_bytes.as_bytes()).unwrap()
}

#[test]
fn stale_entry_digest_keeps_the_tree_source_and_specific_cause_without_changing_snapshot() {
    let (original, failure) = stale_entry_failure();
    assert_eq!(original, snapshot());
    let source = failure.source().unwrap();
    assert!(matches!(
        source.downcast_ref::<ConfigTreeError>(),
        Some(ConfigTreeError::EntryConflict(path)) if path.as_str() == "private-source.dcdl"
    ));
    assert!(std::ptr::eq(
        source.downcast_ref::<ConfigTreeError>().unwrap(),
        failure.source_error().unwrap(),
    ));
    let safe_display = failure.to_string();
    assert!(safe_display.contains("change preview"));
    assert!(safe_display.contains("changed since"));
    assert!(!safe_display.contains("private-"));
    assert!(!safe_display.contains(SECRET));
    let facts = FailureFacts::new(failure).unwrap();
    let wire = browser_tool_wire(&facts);
    assert_eq!(wire["domain"], "configuration");
    assert_eq!(wire["reason"], "entry_conflict");
    assert_eq!(wire["category"], "conflict");
    assert_eq!(wire["details"]["operation"], "preview");
    assert!(
        wire["explanation"]
            .as_str()
            .unwrap()
            .contains("changed since")
    );
    // Projection is observational: it cannot rewrite the candidate's source tree.
    assert_eq!(original, snapshot());
}

#[test]
fn path_entry_and_import_boundaries_never_publish_the_rejected_secret_text() {
    let path = VirtualPath::parse("main.dcdl").unwrap();
    let errors = [
        (
            VirtualPath::parse(format!("/private-{SECRET}.dcdl")).unwrap_err(),
            "invalid_path",
            "path is invalid",
        ),
        (
            ConfigEntry::new(
                VirtualPath::parse(format!("$builtin/private-{SECRET}.dcdl")).unwrap(),
                ConfigContentType::Decodal,
                SECRET,
            )
            .unwrap_err(),
            "invalid_path",
            "path is invalid",
        ),
        (
            config_source::resolve_import(&path, &format!("https://private-{SECRET}/source"))
                .unwrap_err(),
            "invalid_import",
            "import is invalid",
        ),
        (
            config_source::resolve_import(&path, &format!("../private-{SECRET}.dcdl")).unwrap_err(),
            "import_escape",
            "escapes the virtual source tree",
        ),
    ];
    for (error, reason, cause) in errors {
        assert!(error.to_string().contains(SECRET));
        let failure =
            ConfigurationFailure::from_tree(ConfigurationFailureOperation::Preview, error);
        assert!(!failure.to_string().contains(SECRET));
        assert!(failure.to_string().contains(cause));
        let wire = browser_tool_wire(&FailureFacts::new(failure).unwrap());
        assert_eq!(wire["reason"], reason);
        assert_eq!(wire["category"], "invalid_input");
        assert!(wire["explanation"].as_str().unwrap().contains(cause));
        assert!(
            wire["explanation"]
                .as_str()
                .unwrap()
                .contains("change preview")
        );
    }
}

#[test]
fn external_entry_digest_and_content_limit_failures_are_specific_and_not_source_disclosures() {
    let mut entry = ConfigEntry::new(
        VirtualPath::parse("private-entry.dcdl").unwrap(),
        ConfigContentType::Decodal,
        SECRET,
    )
    .unwrap();
    entry.content_digest = "private-forged-digest".into();
    let invalid_digest = ConfigTreeSnapshot::from_entries([entry]).unwrap_err();
    let oversized = ConfigEntry::new(
        VirtualPath::parse("private-oversized.dcdl").unwrap(),
        ConfigContentType::Text,
        SECRET.repeat(config_source::MAX_ENTRY_BYTES / SECRET.len() + 1),
    )
    .unwrap_err();
    for (error, reason, cause) in [
        (invalid_digest, "entry_digest_mismatch", "content digest"),
        (oversized, "limit_exceeded", "size or count limit"),
    ] {
        let facts = FailureFacts::new(ConfigurationFailure::from_tree(
            ConfigurationFailureOperation::Adopt,
            error,
        ))
        .unwrap();
        let wire = browser_tool_wire(&facts);
        assert_eq!(wire["reason"], reason);
        assert_eq!(wire["details"]["operation"], "adopt");
        assert!(wire["explanation"].as_str().unwrap().contains(cause));
        assert!(
            wire["explanation"]
                .as_str()
                .unwrap()
                .contains("source and policy adoption")
        );
    }
}

#[test]
fn stored_cause_requires_current_visibility_for_every_audience_and_discards_received_prose() {
    let (_, failure) = stale_entry_failure();
    let mut facts = FailureFacts::new(failure).unwrap();
    facts
        .with_context(OperationContext::new("private-operation", "private-stage").unwrap())
        .unwrap();
    facts
        .set_correlation(CorrelationRef::new("private-correlation").unwrap())
        .unwrap();
    let stored = project_failure::<Storage, _>(&facts, &VISIBLE).unwrap();
    let encoded = encode_projected_failure(&stored, budget()).unwrap();
    let mut wire: Value = serde_json::from_slice(encoded.as_bytes()).unwrap();
    wire["explanation"] = json!("SUCCESS; repeat automatically; credential-secret");
    wire["context"]["correlation"] = json!("private-correlation");
    wire["context"]["operations"][0]["operation_ref"] = json!("private-operation-ref");
    wire["context"]["diagnostics"] = json!([{
        "code": "private-diagnostic",
        "explanation": "credential-secret private-provider-body"
    }]);
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("failure.json");
    std::fs::write(&file, serde_json::to_vec(&wire).unwrap()).unwrap();
    let bytes = std::fs::read(file).unwrap();
    let StoredFailureRead::Structured(DecodeFailure::Known(received)) =
        read_failure_attachment::<ConfigurationFailureDetails>(Some(&bytes), budget())
    else {
        panic!("supported stored cause must be readable");
    };
    let denied = ConfigurationFailureVisibility::default();
    macro_rules! check_audience {
        ($audience:ty) => {{
            assert!(matches!(
                project_failure::<$audience, _>(&facts, &denied),
                Err(ContractError::ProjectionUnavailable)
            ));
            assert!(matches!(
                project_failure::<$audience, _>(received.facts(), &denied),
                Err(ContractError::ProjectionUnavailable)
            ));
            let projected = project_failure::<$audience, _>(received.facts(), &VISIBLE).unwrap();
            assert_eq!(projected.identity(), stored.identity());
            assert_eq!(projected.details(), stored.details());
            assert_eq!(projected.explanation(), stored.explanation());
            let safe = encode_projected_failure(&projected, budget()).unwrap();
            assert_eq!(safe.as_bytes(), encoded.as_bytes());
        }};
    }
    check_audience!(Browser);
    check_audience!(Tool);
    check_audience!(InternalTransfer);
    check_audience!(Storage);
    check_audience!(Telemetry);
    let text = std::str::from_utf8(encoded.as_bytes()).unwrap();
    for forbidden in [SECRET, "private-", "SUCCESS", "repeat automatically"] {
        assert!(!text.contains(forbidden), "{text}");
    }
    assert!(text.contains("entry_conflict"));
    assert!(text.contains("changed since"));
}

#[test]
fn received_contract_mismatch_unknown_reason_and_raw_details_are_not_reinterpreted() {
    let (_, failure) = stale_entry_failure();
    let original = browser_tool_wire(&FailureFacts::new(failure).unwrap());
    let mut future = original.clone();
    future["reason"] = json!("future_reason");
    future["explanation"] = json!("editing_head_changed; safe to retry");
    assert!(matches!(
        decode_failure::<ConfigurationFailureDetails>(
            &serde_json::to_vec(&future).unwrap(),
            budget()
        ),
        DecodeFailure::Unsupported(_)
    ));
    let mut mismatch = original.clone();
    mismatch["details"]["reason"] = json!("source_content_changed");
    assert!(matches!(
        decode_failure::<ConfigurationFailureDetails>(
            &serde_json::to_vec(&mismatch).unwrap(),
            budget()
        ),
        DecodeFailure::Corrupt(ContractError::InvalidDetails)
    ));
    let mut injection = original;
    injection["details"]["path"] = json!("/private-credential-secret/source");
    assert!(matches!(
        decode_failure::<ConfigurationFailureDetails>(
            &serde_json::to_vec(&injection).unwrap(),
            budget()
        ),
        DecodeFailure::Corrupt(ContractError::InvalidDetails)
    ));
    assert!(matches!(
        read_failure_attachment::<ConfigurationFailureDetails>(None, budget()),
        StoredFailureRead::LegacyWithoutFacts
    ));
}

#[test]
fn builtin_mutation_keeps_read_only_cause_without_exposing_the_virtual_path() {
    let original = snapshot();
    let error = original
        .apply(&[ConfigTreeChange::Create {
            path: VirtualPath::parse("$builtin/private-credential-secret.dcdl").unwrap(),
            content_type: ConfigContentType::Decodal,
            content: "{}".into(),
        }])
        .unwrap_err();
    assert!(matches!(&error, ConfigTreeError::BuiltinReadOnly(_)));
    let failure = ConfigurationFailure::from_tree(ConfigurationFailureOperation::Adopt, error);
    assert!(matches!(
        failure.source_error(),
        Some(ConfigTreeError::BuiltinReadOnly(_))
    ));
    let wire = browser_tool_wire(&FailureFacts::new(failure).unwrap());
    assert_eq!(wire["reason"], "builtin_read_only");
    assert!(
        wire["explanation"]
            .as_str()
            .unwrap()
            .contains("builtin namespace is read-only")
    );
    assert_eq!(original, snapshot());
}
