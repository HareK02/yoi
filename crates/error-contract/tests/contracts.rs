//! Public contract component tests, run by `cargo test -p error-contract` with default features.
//! Production projection/codecs/RFC adapters consume the shared sample domains and JSON fixtures.
//! Authorization, operation outcomes and side effects are fixtures, not production policy or
//! persistence guarantees. No network, process, external service or test-support feature is needed.

use error_contract::*;

#[path = "../src/test_support.rs"]
mod support;

use error_contract::{codec, problem};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{cell::Cell, error::Error, rc::Rc};
use support::*;

fn budget() -> Budget {
    Budget::new(usize::MAX, usize::MAX)
}

fn access(can_view_resource: bool, language: Language) -> CurrentAccess {
    CurrentAccess {
        can_view_resource,
        language,
    }
}

fn subject_json() -> Value {
    serde_json::from_str(SUBJECT_FIXTURE).unwrap()
}

fn known<D: WireDetails>(bytes: &[u8]) -> ReceivedFailure<D> {
    match decode_failure(bytes, budget()) {
        DecodeFailure::Known(received) => received,
        DecodeFailure::Unsupported(_) => panic!("expected a known failure, got unsupported"),
        DecodeFailure::Corrupt(error) => panic!("expected a known failure, got {error:?}"),
    }
}

fn assert_corrupt<D: WireDetails>(bytes: &[u8], expected: ContractError) {
    match decode_failure::<D>(bytes, budget()) {
        DecodeFailure::Corrupt(error) => assert_eq!(error, expected),
        _ => panic!("expected corrupt failure: {expected:?}"),
    }
}

fn assert_subject_value_corrupt(value: &Value, expected: ContractError) {
    assert_corrupt::<SubjectDenied>(&serde_json::to_vec(value).unwrap(), expected);
}

fn assert_unsupported(bytes: &[u8]) {
    match decode_failure::<SubjectDenied>(bytes, budget()) {
        DecodeFailure::Unsupported(opaque) => assert_eq!(opaque.retained_bytes(), bytes.len()),
        _ => panic!("expected opaque unsupported failure"),
    }
}

fn storage_roundtrip<D>(fixture: &str, expected: D, reference_field: &str)
where
    D: WireDetails + PartialEq + std::fmt::Debug,
    D: ProjectFor<Storage, Authorization = CurrentAccess, Details = D>,
    D: ProjectFor<Browser, Authorization = CurrentAccess, Details = D>,
{
    assert_semantic_roundtrip(fixture.as_bytes(), &expected, budget());
    let received = known::<D>(fixture.as_bytes());
    let stored =
        project_failure::<Storage, _>(received.facts(), &access(true, Language::English)).unwrap();
    let encoded = encode_projected_failure(&stored, budget()).unwrap();
    assert_semantic_roundtrip(encoded.as_bytes(), &expected, budget());
    assert_no_fixture_secrets(encoded.as_bytes());

    let restored = known::<D>(encoded.as_bytes());
    let public =
        project_failure::<Browser, _>(restored.facts(), &access(false, Language::Japanese))
            .unwrap();
    let public_bytes = encode_projected_failure(&public, budget()).unwrap();
    let public_json: Value = serde_json::from_slice(public_bytes.as_bytes()).unwrap();
    assert!(public_json["details"][reference_field].is_null());
    assert_eq!(public.identity(), stored.identity());
    assert_ne!(public.explanation(), stored.explanation());
    assert_eq!(
        restored.facts().cause(),
        &expected,
        "reprojection must not redact stored facts in place"
    );
    assert_no_fixture_secrets(public_bytes.as_bytes());
}

#[test]
fn shared_domain_fixtures_roundtrip_and_storage_uses_current_visibility() {
    assert_detail_allowlist(
        SUBJECT_FIXTURE.as_bytes(),
        &["required_operation", "subject_ref"],
    );
    assert_detail_allowlist(WORKDIR_FIXTURE.as_bytes(), &["operation", "workdir_ref"]);
    let received = known::<SubjectDenied>(SUBJECT_FIXTURE.as_bytes());
    let mut facts = FailureFacts::new(received.facts().cause().clone()).unwrap();
    assert_context_preserves_details(
        &mut facts,
        OperationContext::new("worker.create", "register").unwrap(),
    );
    storage_roundtrip(
        SUBJECT_FIXTURE,
        SubjectDenied {
            required_operation: RequiredOperation::SessionRecord,
            subject_ref: Some("subject-1".into()),
        },
        "subject_ref",
    );
    storage_roundtrip(
        WORKDIR_FIXTURE,
        WorkdirInUse {
            operation: WorkdirOperation::Open,
            workdir_ref: Some("workdir-1".into()),
        },
        "workdir_ref",
    );
}

fn assert_source_preserved<D: FailureCause + Error + 'static>(facts: &FailureFacts<D>) {
    let cause = facts.source().expect("concrete domain cause");
    assert!(cause.downcast_ref::<D>().is_some());
    let mut private = cause.source().expect("original source chain");
    while let Some(source) = private.source() {
        private = source;
    }
    assert!(private.downcast_ref::<PrivateSource>().is_some());
    for secret in FIXTURE_SECRETS {
        assert!(
            private.to_string().contains(secret),
            "fixture must exercise private source content"
        );
        assert!(
            !facts.to_string().contains(secret),
            "facts Display must not format source"
        );
    }
}

#[test]
fn projection_and_encoding_preserve_both_concrete_source_chains() {
    let subject = subject_facts();
    let workdir = workdir_facts();
    assert_source_preserved(&subject);
    assert_source_preserved(&workdir);
    let auth = access(false, Language::English);
    let subject_projection = project_failure::<Tool, _>(&subject, &auth).unwrap();
    let workdir_projection = project_failure::<Tool, _>(&workdir, &auth).unwrap();
    assert_no_fixture_secrets(
        encode_projected_failure(&subject_projection, budget())
            .unwrap()
            .as_bytes(),
    );
    assert_no_fixture_secrets(
        encode_projected_failure(&workdir_projection, budget())
            .unwrap()
            .as_bytes(),
    );
    assert_source_preserved(&subject);
    assert_source_preserved(&workdir);
}

fn assert_audience_allowlist<A: Audience>()
where
    SubjectError: ProjectFor<A, Authorization = CurrentAccess, Details = SubjectDenied>,
    WorkdirError: ProjectFor<A, Authorization = CurrentAccess, Details = WorkdirInUse>,
    SubjectDenied: ProjectFor<A, Authorization = CurrentAccess, Details = SubjectDenied>,
    WorkdirInUse: ProjectFor<A, Authorization = CurrentAccess, Details = WorkdirInUse>,
{
    for visible in [false, true] {
        let auth = access(visible, Language::English);
        let subject = project_failure::<A, _>(&subject_facts(), &auth).unwrap();
        let workdir = project_failure::<A, _>(&workdir_facts(), &auth).unwrap();
        for bytes in [
            encode_projected_failure(&subject, budget())
                .unwrap()
                .as_bytes()
                .to_vec(),
            encode_projected_failure(&workdir, budget())
                .unwrap()
                .as_bytes()
                .to_vec(),
        ] {
            assert_no_fixture_secrets(&bytes);
        }

        for (fixture, field, is_subject) in [
            (SUBJECT_FIXTURE, "subject_ref", true),
            (WORKDIR_FIXTURE, "workdir_ref", false),
        ] {
            let mut value: Value = serde_json::from_str(fixture).unwrap();
            value["context"]["operations"][0]["operation_ref"] = json!("visible-operation-ref");
            value["context"]["operations"]
                .as_array_mut()
                .unwrap()
                .push(json!({
                    "operation": "internal-source", "stage": "raw-http-body",
                    "operation_ref": "/private/worktree"
                }));
            value["context"]["diagnostics"] = json!([{
                "code": "internal-source", "explanation": "credential-secret raw-http-body /private/worktree"
            }]);
            value["context"]["causes"] = json!([{
                "domain": "internal-source", "reason": "credential-secret", "correlation": "raw-http-body"
            }]);
            let bytes = serde_json::to_vec(&value).unwrap();
            let output = if is_subject {
                let received = known::<SubjectDenied>(&bytes);
                let projected = project_failure::<A, _>(received.facts(), &auth).unwrap();
                encode_projected_failure(&projected, budget())
                    .unwrap()
                    .as_bytes()
                    .to_vec()
            } else {
                let received = known::<WorkdirInUse>(&bytes);
                let projected = project_failure::<A, _>(received.facts(), &auth).unwrap();
                encode_projected_failure(&projected, budget())
                    .unwrap()
                    .as_bytes()
                    .to_vec()
            };
            assert_no_fixture_secrets(&output);
            let output: Value = serde_json::from_slice(&output).unwrap();
            assert_eq!(output["details"][field].is_string(), visible);
            assert_eq!(
                output["context"]["operations"][0]["operation_ref"].is_string(),
                visible
            );
            assert_eq!(output["context"]["operations"].as_array().unwrap().len(), 1);
            assert_eq!(output["context"]["causes"], json!([]));
            assert_eq!(output["context"]["diagnostics"], json!([]));
            assert_eq!(output["context"]["supplementary_omitted"], true);
            if is_subject {
                assert_eq!(output["details"]["required_operation"], "session.record");
                assert!(
                    output["explanation"]
                        .as_str()
                        .unwrap()
                        .contains("session.record")
                );
            }
        }
    }
}

#[test]
fn every_audience_allowlists_secrets_without_dropping_required_operation() {
    assert_audience_allowlist::<Browser>();
    assert_audience_allowlist::<Tool>();
    assert_audience_allowlist::<Storage>();
    assert_audience_allowlist::<InternalTransfer>();
    assert_audience_allowlist::<Telemetry>();
}

#[test]
fn telemetry_observation_uses_projected_local_context_not_source_text() {
    let facts = subject_facts();
    let projected =
        project_failure::<Telemetry, _>(&facts, &access(false, Language::Japanese)).unwrap();
    let fields = observation_fields(&projected);
    assert_eq!(
        (fields.domain, fields.reason),
        ("subject", "operation_denied")
    );
    assert_eq!(
        (fields.operation, fields.stage),
        (Some("worker.create"), Some("register"))
    );
    assert_eq!(fields.correlation, Some("correlation-1"));
    for secret in FIXTURE_SECRETS {
        assert!(!format!("{fields:?}").contains(secret));
    }
}

#[test]
fn explanation_language_and_supplementary_order_do_not_define_semantics() {
    let mut value = subject_json();
    value["explanation"] = json!("An unrelated received translation; not an identity source.");
    value["context"]["operations"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "operation": "workdir.open", "stage": "prepare", "operation_ref": null
        }));
    value["context"]["diagnostics"] = json!([
        {"code": "first", "explanation": "first detail"},
        {"code": "second", "explanation": "second detail"}
    ]);
    value["context"]["causes"] = json!([
        {"domain": "workdir", "reason": "in_use", "correlation": "cause-1"},
        {"domain": "subject", "reason": "operation_denied", "correlation": "cause-2"}
    ]);
    let original = known::<SubjectDenied>(&serde_json::to_vec(&value).unwrap());
    for key in ["operations", "diagnostics", "causes"] {
        value["context"][key].as_array_mut().unwrap().reverse();
    }
    value["explanation"] = json!("別の言語・順序でも理由を推測しない。");
    let reordered = known::<SubjectDenied>(&serde_json::to_vec(&value).unwrap());
    assert_eq!(original.facts().cause(), reordered.facts().cause());
    assert_ne!(
        original.facts().context().causes,
        reordered.facts().context().causes
    );
    let english =
        project_failure::<Browser, _>(original.facts(), &access(false, Language::English)).unwrap();
    let japanese =
        project_failure::<Browser, _>(reordered.facts(), &access(false, Language::Japanese))
            .unwrap();
    assert_eq!(english.identity(), japanese.identity());
    assert_eq!(english.details(), japanese.details());
    assert_ne!(english.explanation(), japanese.explanation());
    assert_eq!(
        japanese.context().operations,
        reordered.facts().context().operations
    );
}

#[test]
fn byte_limit_is_checked_before_json_parsing_at_32_kib() {
    let mut bytes = SUBJECT_FIXTURE.trim().as_bytes().to_vec();
    bytes.resize(MAX_FAILURE_BYTES, b' ');
    known::<SubjectDenied>(&bytes);
    bytes.push(b' ');
    assert_corrupt::<SubjectDenied>(&bytes, ContractError::TooLarge);

    let invalid = vec![b'x'; MAX_FAILURE_BYTES];
    assert_corrupt::<SubjectDenied>(&invalid, ContractError::Invalid);
    let invalid = vec![b'x'; MAX_FAILURE_BYTES + 1];
    assert_corrupt::<SubjectDenied>(&invalid, ContractError::TooLarge);
}

#[test]
fn depth_16_is_allowed_and_17_is_rejected_before_typed_parsing() {
    for (arrays, expected_depth) in [
        (MAX_JSON_DEPTH - 2, MAX_JSON_DEPTH),
        (MAX_JSON_DEPTH - 1, MAX_JSON_DEPTH + 1),
    ] {
        let mut nested = json!(null);
        for _ in 0..arrays {
            nested = json!([nested]);
        }
        let mut value = subject_json();
        value["reason"] = json!("future_reason");
        value["details"] = json!({"nested": nested});
        let bytes = serde_json::to_vec(&value).unwrap();
        if expected_depth == MAX_JSON_DEPTH {
            assert_unsupported(&bytes);
        } else {
            assert_corrupt::<SubjectDenied>(&bytes, ContractError::TooDeep);
        }
    }
    let mut value = subject_json();
    value["explanation"] = json!("brackets inside escaped text: \\\" [[{{ }}]] \\");
    known::<SubjectDenied>(&serde_json::to_vec(&value).unwrap());
}

#[test]
fn transport_and_enclosing_budgets_both_limit_decoding_and_encoding() {
    let projected =
        project_failure::<Browser, _>(&subject_facts(), &access(false, Language::English)).unwrap();
    let encoded = encode_projected_failure(&projected, budget()).unwrap();
    let len = encoded.as_bytes().len();
    for limits in [(len, usize::MAX), (usize::MAX, len)] {
        let exact = Budget::new(limits.0, limits.1);
        assert!(matches!(
            decode_failure::<SubjectDenied>(encoded.as_bytes(), exact),
            DecodeFailure::Known(_)
        ));
        assert!(encode_projected_failure(&projected, exact).is_ok());
    }
    for limits in [
        (len - 1, usize::MAX),
        (usize::MAX, len - 1),
        (0, len),
        (len, 0),
    ] {
        let short = Budget::new(limits.0, limits.1);
        assert!(matches!(
            decode_failure::<SubjectDenied>(encoded.as_bytes(), short),
            DecodeFailure::Corrupt(ContractError::TooLarge)
        ));
        assert!(matches!(
            encode_projected_failure(&projected, short),
            Err(ContractError::TooLarge)
        ));
    }
}

#[test]
fn transport_and_enclosing_depth_budgets_limit_both_codecs() {
    let projected = browser_subject();
    let encoded = encode_projected_failure(&projected, budget()).unwrap();
    // Envelope/context/operations/operation is the deepest path in this fixture.
    for (transport, remaining) in [(4, MAX_JSON_DEPTH), (MAX_JSON_DEPTH, 4)] {
        let exact = budget().with_depth(transport, remaining);
        assert!(encode_projected_failure(&projected, exact).is_ok());
        assert!(matches!(
            decode_failure::<SubjectDenied>(encoded.as_bytes(), exact),
            DecodeFailure::Known(_)
        ));
    }
    for (transport, remaining) in [(3, MAX_JSON_DEPTH), (MAX_JSON_DEPTH, 3)] {
        let short = budget().with_depth(transport, remaining);
        assert!(matches!(
            encode_projected_failure(&projected, short),
            Err(ContractError::TooDeep)
        ));
        assert!(matches!(
            decode_failure::<SubjectDenied>(encoded.as_bytes(), short),
            DecodeFailure::Corrupt(ContractError::TooDeep)
        ));
    }
    // RFC's wrapper consumes another depth level, not a fresh independent allowance.
    let response =
        problem::to_problem_details(&projected, &WorkerApi, "/requests/1", budget()).unwrap();
    let exact = budget().with_depth(5, 5);
    assert!(problem::to_problem_details(&projected, &WorkerApi, "/requests/1", exact).is_ok());
    assert!(matches!(
        problem::decode_problem_details::<SubjectDenied>(
            response.body(),
            409,
            response.content_type,
            &WorkerApi,
            exact
        ),
        DecodeFailure::Known(_)
    ));
    let short = budget().with_depth(4, 4);
    assert!(matches!(
        problem::to_problem_details(&projected, &WorkerApi, "/requests/1", short),
        Err(ContractError::TooDeep)
    ));
    assert!(matches!(
        problem::decode_problem_details::<SubjectDenied>(
            response.body(),
            409,
            response.content_type,
            &WorkerApi,
            short
        ),
        DecodeFailure::Corrupt(ContractError::TooDeep)
    ));
}

#[test]
fn collection_limits_accept_exactly_8_contexts_4_causes_8_diagnostics() {
    for (field, limit, entry) in [
        (
            "operations",
            MAX_CONTEXTS,
            json!({"operation": "worker.create", "stage": "register", "operation_ref": null}),
        ),
        (
            "causes",
            MAX_CAUSES,
            json!({"domain": "subject", "reason": "operation_denied", "correlation": "cause-1"}),
        ),
        (
            "diagnostics",
            MAX_DIAGNOSTICS,
            json!({"code": "diagnostic", "explanation": "safe explanation"}),
        ),
    ] {
        for count in [limit, limit + 1] {
            let mut value = subject_json();
            value["context"][field] = Value::Array(vec![entry.clone(); count]);
            if count == limit {
                let received = known::<SubjectDenied>(&serde_json::to_vec(&value).unwrap());
                let received_context = serde_json::to_value(received.facts().context()).unwrap();
                assert_eq!(received_context[field], value["context"][field]);
            } else {
                assert_subject_value_corrupt(&value, ContractError::Invalid);
            }
        }
    }
}

#[test]
fn rejected_context_and_causal_appends_leave_original_facts_intact() {
    let mut facts = subject_facts();
    while facts.context().operations.len() < MAX_CONTEXTS {
        facts
            .with_context(OperationContext::new("worker.create", "register").unwrap())
            .unwrap();
    }
    for index in 0..MAX_CAUSES {
        facts
            .with_causal_link(CausalLink {
                domain: "subject".into(),
                reason: "operation_denied".into(),
                correlation: CorrelationRef::new(format!("cause-{index}")).unwrap(),
            })
            .unwrap();
    }
    let before = facts.context().clone();
    assert_eq!(
        facts.with_context(OperationContext::new("workdir.open", "prepare").unwrap()),
        Err(ContractError::Invalid)
    );
    assert_eq!(
        facts.with_causal_link(before.causes[0].clone()),
        Err(ContractError::Invalid)
    );
    assert_eq!(
        facts.with_context(OperationContext {
            operation: "".into(),
            stage: "register".into(),
            operation_ref: None
        }),
        Err(ContractError::Invalid)
    );
    assert_eq!(facts.context(), &before);
    assert_source_preserved(&facts);
}

#[test]
fn identifiers_are_bounded_by_128_utf8_bytes_at_every_received_position() {
    for pointer in [
        "/domain",
        "/reason",
        "/context/operations/0/operation",
        "/context/operations/0/stage",
        "/context/operations/0/operation_ref",
        "/context/correlation",
        "/context/causes/0/domain",
        "/context/causes/0/reason",
        "/context/causes/0/correlation",
        "/context/diagnostics/0/code",
        "/required_extensions/0",
        "/details/subject_ref",
    ] {
        for text in [
            "a".repeat(MAX_IDENTIFIER_BYTES),
            "é".repeat(MAX_IDENTIFIER_BYTES / 2),
            format!("{}a", "é".repeat(MAX_IDENTIFIER_BYTES / 2)),
            "".into(),
            "bad\nidentifier".into(),
        ] {
            let mut value = subject_json();
            value["context"]["causes"] = json!([{"domain": "subject", "reason": "operation_denied", "correlation": "cause-1"}]);
            value["context"]["diagnostics"] =
                json!([{"code": "diagnostic", "explanation": "safe"}]);
            if pointer.starts_with("/required_extensions") {
                value["required_extensions"] = json!(["future.extension"]);
            }
            *value.pointer_mut(pointer).unwrap() = json!(text);
            let bytes = serde_json::to_vec(&value).unwrap();
            if text.len() == MAX_IDENTIFIER_BYTES {
                if matches!(pointer, "/domain" | "/reason" | "/required_extensions/0") {
                    assert_unsupported(&bytes);
                } else {
                    known::<SubjectDenied>(&bytes);
                }
            } else {
                assert_corrupt::<SubjectDenied>(&bytes, ContractError::Invalid);
            }
        }
    }
}

#[test]
fn explanation_limits_use_2048_utf8_bytes_and_never_truncate() {
    for pointer in ["/explanation", "/context/diagnostics/0/explanation"] {
        for text in [
            "a".repeat(MAX_EXPLANATION_BYTES),
            "é".repeat(MAX_EXPLANATION_BYTES / 2),
            format!("{}a", "é".repeat(MAX_EXPLANATION_BYTES / 2)),
            "".into(),
            "bad\0explanation".into(),
        ] {
            let mut value = subject_json();
            value["context"]["diagnostics"] =
                json!([{"code": "diagnostic", "explanation": "safe"}]);
            *value.pointer_mut(pointer).unwrap() = json!(text);
            if text.len() == MAX_EXPLANATION_BYTES {
                known::<SubjectDenied>(&serde_json::to_vec(&value).unwrap());
            } else {
                assert_subject_value_corrupt(&value, ContractError::Invalid);
            }
        }
    }
}

#[test]
fn future_versions_reasons_and_required_extensions_remain_opaque() {
    for (field, future) in [
        ("version", json!(SCHEMA_VERSION + 1)),
        ("details_version", json!(2)),
        ("reason", json!("future_reason")),
        ("domain", json!("future_domain")),
        ("required_extensions", json!(["future.extension"])),
        (
            "required_extensions",
            json!(vec!["future.extension"; MAX_DIAGNOSTICS]),
        ),
    ] {
        let mut value = subject_json();
        value[field] = future;
        // Unknown data must not be interpreted using the currently known typed details.
        value["details"] = json!({"future_payload": [1, 2, 3]});
        assert_unsupported(&serde_json::to_vec(&value).unwrap());
    }
    let mut value = subject_json();
    value["required_extensions"] = json!(vec!["future.extension"; MAX_DIAGNOSTICS + 1]);
    assert_subject_value_corrupt(&value, ContractError::Invalid);
}

#[test]
fn invalid_known_typed_details_are_corrupt_not_unsupported() {
    for details in [
        json!({}),
        json!({"required_operation": "session.delete", "subject_ref": null}),
        json!({"required_operation": "session.record", "subject_ref": 7}),
        json!({"required_operation": "session.record", "subject_ref": null, "credential": "credential-secret"}),
        json!(null),
    ] {
        let mut value = subject_json();
        value["details"] = details;
        assert_subject_value_corrupt(&value, ContractError::InvalidDetails);
    }
    let mut value = subject_json();
    value["category"] = json!("conflict");
    assert_subject_value_corrupt(&value, ContractError::InvalidDetails);
    for field in ["version", "details_version"] {
        let mut value = subject_json();
        value[field] = json!(0);
        assert_subject_value_corrupt(&value, ContractError::Invalid);
    }
    let mut value: Value = serde_json::from_str(WORKDIR_FIXTURE).unwrap();
    value["details"]["operation"] = json!("delete");
    assert_corrupt::<WorkdirInUse>(
        &serde_json::to_vec(&value).unwrap(),
        ContractError::InvalidDetails,
    );
}

fn browser_subject() -> ProjectedFailure<Browser, SubjectDenied> {
    project_failure::<Browser, _>(&subject_facts(), &access(false, Language::English)).unwrap()
}

fn assert_problem_corrupt(value: &Value, status: u16, content_type: &str, expected: ContractError) {
    match problem::decode_problem_details::<SubjectDenied>(
        &serde_json::to_vec(value).unwrap(),
        status,
        content_type,
        &WorkerApi,
        budget(),
    ) {
        DecodeFailure::Corrupt(error) => assert_eq!(error, expected),
        _ => panic!("expected corrupt problem details"),
    }
}

#[test]
fn api_boundary_chooses_inner_403_and_outer_409_without_forwarding_status() {
    let projected = browser_subject();
    let inner =
        problem::to_problem_details(&projected, &SubjectApi, "/requests/inner", budget()).unwrap();
    let outer =
        problem::to_problem_details(&projected, &WorkerApi, "/requests/outer", budget()).unwrap();
    assert_eq!((inner.status, outer.status), (403, 409));
    for response in [&inner, &outer] {
        assert_eq!(response.content_type, "application/problem+json");
        let body: Value = serde_json::from_slice(response.body()).unwrap();
        assert_eq!(body["status"], response.status);
        assert_eq!(body["detail"], body["yoi_failure"]["explanation"]);
        assert_eq!(
            body["yoi_failure"]["details"]["required_operation"],
            "session.record"
        );
        assert_no_fixture_secrets(response.body());
    }
    match problem::decode_problem_details::<SubjectDenied>(
        inner.body(),
        403,
        inner.content_type,
        &SubjectApi,
        budget(),
    ) {
        DecodeFailure::Known(received) => assert_eq!(received.facts().cause(), projected.details()),
        _ => panic!("inner API contract must roundtrip"),
    }
    match problem::decode_problem_details::<SubjectDenied>(
        outer.body(),
        409,
        outer.content_type,
        &WorkerApi,
        budget(),
    ) {
        DecodeFailure::Known(received) => assert_eq!(received.facts().cause(), projected.details()),
        _ => panic!("outer API contract must roundtrip"),
    }
    let inner_json: Value = serde_json::from_slice(inner.body()).unwrap();
    assert_problem_corrupt(&inner_json, 403, inner.content_type, ContractError::Invalid);

    let workdir =
        project_failure::<Browser, _>(&workdir_facts(), &access(false, Language::English)).unwrap();
    let response =
        problem::to_problem_details(&workdir, &WorkdirApi, "/requests/workdir", budget()).unwrap();
    assert!(matches!(
        problem::decode_problem_details::<WorkdirInUse>(
            response.body(),
            409,
            response.content_type,
            &WorkdirApi,
            budget()
        ),
        DecodeFailure::Known(_)
    ));
}

#[test]
fn rfc_extensions_are_ignored_but_status_content_type_and_classification_must_agree() {
    let response =
        problem::to_problem_details(&browser_subject(), &WorkerApi, "/requests/1", budget())
            .unwrap();
    let mut value: Value = serde_json::from_slice(response.body()).unwrap();
    value["future_rfc_extension"] = json!({"arbitrary": ["ignored", 1]});
    let bytes = serde_json::to_vec(&value).unwrap();
    assert!(matches!(
        problem::decode_problem_details::<SubjectDenied>(
            &bytes,
            409,
            "Application/Problem+Json; charset=utf-8",
            &WorkerApi,
            budget()
        ),
        DecodeFailure::Known(_)
    ));
    for (status, content_type) in [
        (403, "application/problem+json"),
        (409, "application/json"),
        (409, "text/plain"),
    ] {
        assert_problem_corrupt(&value, status, content_type, ContractError::Invalid);
    }
    for (pointer, replacement) in [
        ("/status", json!(200)),
        ("/status", json!(403)),
        ("/type", json!("urn:yoi:problem:authorization-denied")),
        ("/title", json!("Authorization denied")),
        ("/detail", json!("disagrees with failure explanation")),
        ("/instance", json!("not a URI reference")),
    ] {
        let mut corrupt = value.clone();
        *corrupt.pointer_mut(pointer).unwrap() = replacement;
        assert_problem_corrupt(
            &corrupt,
            409,
            "application/problem+json",
            ContractError::Invalid,
        );
    }
    let mut unknown = value.clone();
    unknown["yoi_failure"]["required_extensions"] = json!(["future.extension"]);
    assert!(matches!(
        problem::decode_problem_details::<SubjectDenied>(
            &serde_json::to_vec(&unknown).unwrap(),
            409,
            response.content_type,
            &WorkerApi,
            budget()
        ),
        DecodeFailure::Unsupported(_)
    ));
    let mut missing = value;
    missing.as_object_mut().unwrap().remove("yoi_failure");
    assert_problem_corrupt(&missing, 409, response.content_type, ContractError::Invalid);
}

#[test]
fn problem_encoding_and_decoding_obey_the_remaining_transport_budget() {
    let projected = browser_subject();
    let response =
        problem::to_problem_details(&projected, &WorkerApi, "/requests/1", budget()).unwrap();
    let len = response.body().len();
    for limits in [(len, usize::MAX), (usize::MAX, len)] {
        let exact = Budget::new(limits.0, limits.1);
        assert!(problem::to_problem_details(&projected, &WorkerApi, "/requests/1", exact).is_ok());
        assert!(matches!(
            problem::decode_problem_details::<SubjectDenied>(
                response.body(),
                409,
                response.content_type,
                &WorkerApi,
                exact
            ),
            DecodeFailure::Known(_)
        ));
    }
    for limits in [(len - 1, usize::MAX), (usize::MAX, len - 1)] {
        let short = Budget::new(limits.0, limits.1);
        assert!(matches!(
            problem::to_problem_details(&projected, &WorkerApi, "/requests/1", short),
            Err(ContractError::TooLarge)
        ));
        assert!(matches!(
            problem::decode_problem_details::<SubjectDenied>(
                response.body(),
                409,
                response.content_type,
                &WorkerApi,
                short
            ),
            DecodeFailure::Corrupt(ContractError::TooLarge)
        ));
    }
}

#[test]
fn duplicate_envelope_context_and_typed_detail_fields_are_rejected() {
    let input = SUBJECT_FIXTURE.trim();
    for (needle, duplicate, expected) in [
        (
            "\"version\":1",
            "\"version\":1,\"version\":1",
            ContractError::Invalid,
        ),
        (
            "\"required_extensions\":[]",
            "\"required_extensions\":[],\"required_extensions\":[]",
            ContractError::Invalid,
        ),
        (
            "\"diagnostics\":[]",
            "\"diagnostics\":[],\"diagnostics\":[]",
            ContractError::Invalid,
        ),
        (
            "\"reason\":\"operation_denied\"",
            "\"reason\":\"operation_denied\",\"reason\":\"operation_denied\"",
            ContractError::Invalid,
        ),
        (
            "\"stage\":\"register\"",
            "\"stage\":\"register\",\"stage\":\"register\"",
            ContractError::Invalid,
        ),
        (
            "\"required_operation\":\"session.record\"",
            "\"required_operation\":\"session.record\",\"required_operation\":\"session.record\"",
            ContractError::InvalidDetails,
        ),
    ] {
        assert!(input.contains(needle));
        assert_corrupt::<SubjectDenied>(input.replacen(needle, duplicate, 1).as_bytes(), expected);
    }
    let input = WORKDIR_FIXTURE.replace(
        "\"operation\":\"open\"",
        "\"operation\":\"open\",\"operation\":\"open\"",
    );
    assert_corrupt::<WorkdirInUse>(input.as_bytes(), ContractError::InvalidDetails);
}

#[test]
fn duplicate_problem_fields_and_nested_typed_fields_are_rejected() {
    let response =
        problem::to_problem_details(&browser_subject(), &WorkerApi, "/requests/1", budget())
            .unwrap();
    let input = std::str::from_utf8(response.body()).unwrap();
    for (needle, duplicate, expected) in [
        (
            "\"status\":409",
            "\"status\":409,\"status\":409",
            ContractError::Invalid,
        ),
        (
            "\"required_operation\":\"session.record\"",
            "\"required_operation\":\"session.record\",\"required_operation\":\"session.record\"",
            ContractError::InvalidDetails,
        ),
    ] {
        assert!(input.contains(needle));
        let bytes = input.replacen(needle, duplicate, 1);
        match problem::decode_problem_details::<SubjectDenied>(
            bytes.as_bytes(),
            409,
            response.content_type,
            &WorkerApi,
            budget(),
        ) {
            DecodeFailure::Corrupt(error) => assert_eq!(error, expected),
            _ => panic!("duplicate fields must not be normalized away"),
        }
    }
}

struct DisclosureFixture {
    explanation: String,
    context: FailureContext,
    details: SubjectDenied,
    replace_identity: bool,
}

impl FailureCause for DisclosureFixture {
    fn identity(&self) -> FailureIdentity {
        if self.replace_identity {
            WorkdirInUse {
                operation: WorkdirOperation::Open,
                workdir_ref: None,
            }
            .identity()
        } else {
            self.details.identity()
        }
    }

    fn validate(&self) -> Result<(), ContractError> {
        Ok(())
    }
}

impl ProjectFor<Browser> for DisclosureFixture {
    type Authorization = ();
    type Details = SubjectDenied;

    fn disclose(
        &self,
        _: &FailureFacts<Self>,
        _: &(),
    ) -> Result<Disclosure<Self::Details>, ContractError> {
        Ok(Disclosure {
            details: self.details.clone(),
            explanation: self.explanation.clone(),
            context: self.context.clone(),
        })
    }
}

fn disclosure_fixture() -> DisclosureFixture {
    DisclosureFixture {
        explanation: "safe explanation".into(),
        context: FailureContext::default(),
        details: SubjectDenied {
            required_operation: RequiredOperation::SessionRecord,
            subject_ref: None,
        },
        replace_identity: false,
    }
}

#[test]
fn domain_projection_cannot_change_identity_or_bypass_disclosure_validation() {
    for case in [
        "identity",
        "details",
        "explanation",
        "context",
        "causes",
        "diagnostics",
    ] {
        let mut fixture = disclosure_fixture();
        match case {
            "identity" => fixture.replace_identity = true,
            "details" => {
                fixture.details.subject_ref = Some("é".repeat(MAX_IDENTIFIER_BYTES / 2 + 1))
            }
            "explanation" => fixture.explanation = "é".repeat(MAX_EXPLANATION_BYTES / 2 + 1),
            "context" => {
                fixture.context.operations = vec![
                    OperationContext::new("worker.create", "register")
                        .unwrap();
                    MAX_CONTEXTS + 1
                ]
            }
            "causes" => {
                fixture.context.causes = vec![
                    CausalLink {
                        domain: "subject".into(),
                        reason: "operation_denied".into(),
                        correlation: CorrelationRef::new("safe-correlation").unwrap(),
                    };
                    MAX_CAUSES + 1
                ]
            }
            "diagnostics" => {
                fixture.context.diagnostics = vec![
                    Diagnostic {
                        code: "safe".into(),
                        explanation: "safe explanation".into(),
                    };
                    MAX_DIAGNOSTICS + 1
                ]
            }
            _ => unreachable!(),
        }
        let facts = FailureFacts::new(fixture).unwrap();
        let expected = if case == "identity" {
            ContractError::InvalidDetails
        } else {
            ContractError::Invalid
        };
        assert!(
            matches!(project_failure::<Browser, _>(&facts, &()), Err(error) if error == expected),
            "case {case}"
        );
    }
    let mut fixture = disclosure_fixture();
    fixture.explanation = "é".repeat(MAX_EXPLANATION_BYTES / 2);
    fixture.details.subject_ref = Some("é".repeat(MAX_IDENTIFIER_BYTES / 2));
    let facts = FailureFacts::new(fixture).unwrap();
    let projected = project_failure::<Browser, _>(&facts, &()).unwrap();
    let encoded = encode_projected_failure(&projected, budget()).unwrap();
    assert_semantic_roundtrip(encoded.as_bytes(), projected.details(), budget());
    let value: Value = serde_json::from_slice(encoded.as_bytes()).unwrap();
    assert_eq!(value["explanation"], projected.explanation());
}

#[test]
fn json_escaping_cannot_exceed_32_kib_or_silently_drop_required_details() {
    let mut fixture = disclosure_fixture();
    fixture.context.diagnostics = vec![
        Diagnostic {
            code: "safe".into(),
            explanation: "\u{1}".repeat(MAX_EXPLANATION_BYTES),
        };
        MAX_DIAGNOSTICS
    ];
    let facts = FailureFacts::new(fixture).unwrap();
    let projected = project_failure::<Browser, _>(&facts, &()).unwrap();
    assert!(matches!(
        encode_projected_failure(&projected, budget()),
        Err(ContractError::TooLarge)
    ));
    let fallback = codec::encode_or_fallback(Ok(&projected), budget(), None).unwrap();
    assert_eq!(fallback.conversion_error, Some(ContractError::TooLarge));
    assert_eq!(fallback.encoded.as_bytes(), codec::STATIC_FALLBACK);
    assert_eq!(
        projected.context().diagnostics,
        facts.cause().context.diagnostics
    );
}

#[derive(Deserialize)]
#[serde(transparent)]
struct UnencodableDetails {
    details: SubjectDenied,
    #[serde(skip)]
    attempts: Rc<Cell<usize>>,
}

impl Serialize for UnencodableDetails {
    fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        self.attempts.set(self.attempts.get() + 1);
        Err(serde::ser::Error::custom(
            "internal-source credential-secret raw-http-body /private/worktree",
        ))
    }
}

impl FailureCause for UnencodableDetails {
    fn identity(&self) -> FailureIdentity {
        self.details.identity()
    }
    fn validate(&self) -> Result<(), ContractError> {
        self.details.validate()
    }
}

impl WireDetails for UnencodableDetails {
    fn supports(domain: &str, reason: &str, version: u16) -> bool {
        SubjectDenied::supports(domain, reason, version)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("fixture operation denied")]
struct BrokenAdapter {
    #[source]
    source: SubjectError,
    projection_fails: bool,
    projection_attempts: Rc<Cell<usize>>,
    encoding_attempts: Rc<Cell<usize>>,
}

impl FailureCause for BrokenAdapter {
    fn identity(&self) -> FailureIdentity {
        self.source.identity()
    }
    fn validate(&self) -> Result<(), ContractError> {
        self.source.validate()
    }
}

impl ProjectFor<Browser> for BrokenAdapter {
    type Authorization = ();
    type Details = UnencodableDetails;
    fn disclose(
        &self,
        _: &FailureFacts<Self>,
        _: &(),
    ) -> Result<Disclosure<Self::Details>, ContractError> {
        self.projection_attempts
            .set(self.projection_attempts.get() + 1);
        if self.projection_fails {
            Err(ContractError::ProjectionUnavailable)
        } else {
            Ok(Disclosure::new(
                UnencodableDetails {
                    details: self.source.details.clone(),
                    attempts: self.encoding_attempts.clone(),
                },
                "session.record denied",
            ))
        }
    }
}

#[test]
fn conversion_failures_fall_back_without_changing_operation_or_secondary_results() {
    // These are domain-owned results, deliberately outside the failure contract's causal chain.
    struct OperationFixture {
        primary: Result<&'static str, FailureFacts<BrokenAdapter>>,
        committed_effects: Vec<&'static str>,
        cleanup: Result<(), &'static str>,
        audit: Result<(), &'static str>,
        delivery: Result<(), &'static str>,
    }
    for projection_fails in [true, false] {
        let projection_attempts = Rc::new(Cell::new(0));
        let encoding_attempts = Rc::new(Cell::new(0));
        let mut primary = FailureFacts::new(BrokenAdapter {
            source: SubjectError {
                details: subject_facts().cause().details.clone(),
                source: PrivateSource,
            },
            projection_fails,
            projection_attempts: projection_attempts.clone(),
            encoding_attempts: encoding_attempts.clone(),
        })
        .unwrap();
        primary
            .with_context(OperationContext::new("worker.create", "register").unwrap())
            .unwrap();
        primary
            .set_correlation(CorrelationRef::new("safe-correlation").unwrap())
            .unwrap();
        let operation = OperationFixture {
            primary: Err(primary),
            committed_effects: vec!["session-created"],
            cleanup: Err("cleanup-failed"),
            audit: Err("audit-failed"),
            delivery: Err("response-lost"),
        };
        let primary = operation.primary.as_ref().unwrap_err();
        let original_context = primary.context().clone();
        let projection = project_failure::<Browser, _>(primary, &());
        let encoded = codec::encode_or_fallback(
            projection.as_ref().map_err(|error| *error),
            budget(),
            primary.context().correlation.clone(),
        )
        .unwrap();
        assert_eq!(encoded.encoded.as_bytes(), codec::STATIC_FALLBACK);
        assert_eq!(
            encoded.conversion_error,
            Some(if projection_fails {
                ContractError::ProjectionUnavailable
            } else {
                ContractError::Encoding
            })
        );
        assert_eq!(encoded.correlation, primary.context().correlation);
        assert_no_fixture_secrets(encoded.encoded.as_bytes());
        assert_eq!(
            projection_attempts.get(),
            1,
            "fallback must not recursively reproject"
        );
        assert_eq!(
            encoding_attempts.get(),
            usize::from(!projection_fails),
            "fallback must not retry serialization"
        );
        assert_eq!(operation.committed_effects, ["session-created"]);
        assert_eq!(
            (operation.cleanup, operation.audit, operation.delivery),
            (
                Err("cleanup-failed"),
                Err("audit-failed"),
                Err("response-lost")
            )
        );
        assert_eq!(primary.context(), &original_context);
        assert!(
            primary.context().causes.is_empty(),
            "secondary failures are not replacement root causes"
        );
        assert_source_preserved(primary);
    }
}

#[test]
fn fallback_and_response_loss_do_not_turn_committed_success_into_failure() {
    let primary = Ok::<_, &'static str>("resource-created");
    let committed_effects = vec!["resource-created"];
    let delivery = Err::<(), _>("response-lost");
    let boundary = codec::encode_or_fallback::<Browser, SubjectDenied>(
        Err(ContractError::ProjectionUnavailable),
        budget(),
        None,
    )
    .unwrap();
    assert_eq!(boundary.encoded.as_bytes(), codec::STATIC_FALLBACK);
    assert_eq!(primary, Ok("resource-created"));
    assert_eq!(committed_effects, ["resource-created"]);
    assert_eq!(delivery, Err("response-lost"));
}

#[test]
fn static_fallbacks_cannot_bypass_an_exhausted_enclosing_depth_budget() {
    for (transport, remaining) in [(1, MAX_JSON_DEPTH), (MAX_JSON_DEPTH, 1)] {
        let exact = budget().with_depth(transport, remaining);
        let encoded = codec::encode_or_fallback::<Browser, SubjectDenied>(
            Err(ContractError::ProjectionUnavailable),
            exact,
            None,
        )
        .unwrap();
        assert_eq!(encoded.encoded.as_bytes(), codec::STATIC_FALLBACK);
        assert!(problem::problem_fallback(exact).is_ok());
    }
    for (transport, remaining) in [(0, MAX_JSON_DEPTH), (MAX_JSON_DEPTH, 0)] {
        let exhausted = budget().with_depth(transport, remaining);
        assert!(matches!(
            codec::encode_or_fallback::<Browser, SubjectDenied>(
                Err(ContractError::ProjectionUnavailable),
                exhausted,
                None,
            ),
            Err(ContractError::TooDeep)
        ));
        assert!(matches!(
            problem::problem_fallback(exhausted),
            Err(ContractError::TooDeep)
        ));
    }
}

#[test]
fn fallback_is_static_and_fails_closed_when_even_its_exact_budget_does_not_fit() {
    let projected = browser_subject();
    let ordinary = codec::encode_or_fallback(Ok(&projected), budget(), None).unwrap();
    assert!(ordinary.conversion_error.is_none());
    assert_semantic_roundtrip(ordinary.encoded.as_bytes(), projected.details(), budget());
    for limits in [
        (codec::STATIC_FALLBACK.len(), usize::MAX),
        (usize::MAX, codec::STATIC_FALLBACK.len()),
    ] {
        let exact = Budget::new(limits.0, limits.1);
        let result = codec::encode_or_fallback(
            Ok(&projected),
            exact,
            Some(CorrelationRef::new("safe-correlation").unwrap()),
        )
        .unwrap();
        assert_eq!(result.encoded.as_bytes(), codec::STATIC_FALLBACK);
        assert_eq!(result.conversion_error, Some(ContractError::TooLarge));
        assert!(
            !std::str::from_utf8(result.encoded.as_bytes())
                .unwrap()
                .contains("safe-correlation")
        );
    }
    for limits in [
        (codec::STATIC_FALLBACK.len() - 1, usize::MAX),
        (usize::MAX, codec::STATIC_FALLBACK.len() - 1),
    ] {
        assert!(matches!(
            codec::encode_or_fallback::<Browser, SubjectDenied>(
                Err(ContractError::ProjectionUnavailable),
                Budget::new(limits.0, limits.1),
                None
            ),
            Err(ContractError::TooLarge)
        ));
    }
    let len = problem::STATIC_PROBLEM_FALLBACK.len();
    for limits in [(len, usize::MAX), (usize::MAX, len)] {
        let response = problem::problem_fallback(Budget::new(limits.0, limits.1)).unwrap();
        assert_eq!(response.status, 500);
        assert_eq!(response.content_type, "application/problem+json");
        assert_eq!(response.body(), problem::STATIC_PROBLEM_FALLBACK);
        assert_no_fixture_secrets(response.body());
    }
    assert!(matches!(
        problem::problem_fallback(Budget::new(len - 1, len)),
        Err(ContractError::TooLarge)
    ));
    assert!(matches!(
        problem::problem_fallback(Budget::new(len, len - 1)),
        Err(ContractError::TooLarge)
    ));
}

#[test]
fn reader_first_attachment_distinguishes_absence_from_corruption_without_parsing_legacy_text() {
    use error_contract::compatibility::*;
    let legacy =
        json!({"message": "403 session.record unknown", "diagnostics": ["arbitrary old text"]});
    assert!(legacy.get("failure").is_none());
    assert!(matches!(
        read_failure_attachment::<SubjectDenied>(None, budget()),
        StoredFailureRead::LegacyWithoutFacts
    ));
    assert!(matches!(
        read_failure_attachment::<SubjectDenied>(Some(b"old arbitrary error text"), budget()),
        StoredFailureRead::Structured(DecodeFailure::Corrupt(_))
    ));
    assert!(matches!(
        read_failure_attachment::<SubjectDenied>(Some(SUBJECT_FIXTURE.as_bytes()), budget()),
        StoredFailureRead::Structured(DecodeFailure::Known(_))
    ));
    let mut future = subject_json();
    future["version"] = json!(2);
    future["category"] = json!("future_category");
    assert_unsupported(&serde_json::to_vec(&future).unwrap());
    let mut future_problem: Value = serde_json::from_slice(
        problem::to_problem_details(
            &browser_subject(),
            &WorkerApi,
            "urn:yoi:occurrence:test",
            budget(),
        )
        .unwrap()
        .body(),
    )
    .unwrap();
    future_problem["yoi_failure"] = future;
    assert!(matches!(
        problem::decode_problem_details::<SubjectDenied>(
            &serde_json::to_vec(&future_problem).unwrap(),
            409,
            "application/problem+json",
            &WorkerApi,
            budget()
        ),
        DecodeFailure::Unsupported(_)
    ));
}

#[test]
fn untrusted_wire_references_and_correlations_are_not_local_authority_even_when_visible() {
    let mut wire = subject_json();
    wire["details"]["subject_ref"] = json!("credential-secret");
    wire["context"]["correlation"] = json!("/private/worktree");
    wire["context"]["operations"][0]["operation_ref"] = json!("raw-http-body");
    let received = known::<SubjectDenied>(&serde_json::to_vec(&wire).unwrap());
    let auth = access(true, Language::English);
    let projected = project_failure::<Browser, _>(received.facts(), &auth).unwrap();
    let bytes = encode_projected_failure(&projected, budget()).unwrap();
    assert_no_fixture_secrets(bytes.as_bytes());
    assert!(projected.details().subject_ref.is_none());
    assert!(projected.context().correlation.is_none());
    assert!(projected.context().operations[0].operation_ref.is_none());
    assert_eq!(
        projected.details().required_operation,
        RequiredOperation::SessionRecord
    );
}

#[test]
fn explanation_composition_and_unknown_local_presentation_keep_required_information_bounded() {
    let explanation = compose_explanation("session.record denied", "Worker cannot start").unwrap();
    assert!(explanation.contains("session.record"));
    assert!(compose_explanation(&"x".repeat(MAX_EXPLANATION_BYTES), "impact").is_err());
    let presentation = unavailable_presentation(
        "register",
        Some(CorrelationRef::new("correlation-1").unwrap()),
    )
    .unwrap();
    assert_eq!(presentation.stage, "register");
    assert_eq!(presentation.correlation.unwrap().as_str(), "correlation-1");
}

#[cfg(feature = "json-schema")]
#[test]
fn schemas_derive_typed_details_and_shared_context_bounds_from_the_same_wire_types() {
    let subject =
        serde_json::to_value(error_contract::schema::failure_schema::<SubjectDenied>()).unwrap();
    let workdir =
        serde_json::to_value(error_contract::schema::problem_schema::<WorkdirInUse>()).unwrap();
    assert_eq!(subject["properties"]["version"]["minimum"], 1);
    assert_eq!(
        subject["$defs"]["FailureContext"]["properties"]["operations"]["maxItems"],
        MAX_CONTEXTS
    );
    assert_eq!(
        subject["$defs"]["SubjectDenied"]["additionalProperties"],
        false
    );
    assert_eq!(
        subject["$defs"]["RequiredOperation"]["enum"],
        json!(["session.record"])
    );
    assert!(workdir["$defs"]["WorkdirInUse"]["properties"]["operation"].is_object());
}
