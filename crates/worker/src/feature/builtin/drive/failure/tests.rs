use super::{
    DriveClientFailure, DriveClientFailureDetails as Details,
    DriveClientFailureOperation as Operation, DriveClientFailureReason as Reason,
    DriveClientFailureVisibility as Visibility,
};
use crate::feature::builtin::drive::backend::DriveError;
use error_contract::{
    Browser, Budget, Category, ContractError, CorrelationRef, DecodeFailure, FailureCause,
    FailureFacts, InternalTransfer, OperationContext, Storage, Telemetry, Tool, WireDetails,
    decode_failure, encode_projected_failure, observation_fields, project_failure,
};
use serde_json::{Value, json};
use server_api::authorization::PermissionResource;
use server_api::{DriveEntryRef, DriveMutation};

const VISIBLE: Visibility = Visibility {
    can_view_cause: true,
};
const PRIVATE: &str = "credential-secret /private/path provider-body HTTP status 503 retry-safe";

fn budget() -> Budget {
    Budget::new(4096, 4096)
}

fn authorization(denied: bool) -> DriveError {
    DriveError::Authorization {
        operation: PRIVATE.into(),
        resource: PermissionResource::DriveRequest {
            workspace_id: "workspaceID-secret".into(),
            request_id: "requestID-secret".into(),
        },
        denied,
    }
}

#[test]
fn actual_enum_mapping_retains_source_but_discloses_only_closed_details_for_all_audiences() {
    let cases = [
        (
            authorization(true),
            Reason::AuthorizationDenied,
            Category::Authorization,
        ),
        (
            authorization(false),
            Reason::AuthorizationFailed,
            Category::Authorization,
        ),
        (DriveError::Denied, Reason::Denied, Category::Authorization),
        (
            DriveError::NotFound,
            Reason::NotFound,
            Category::InvalidInput,
        ),
        (DriveError::Conflict, Reason::Conflict, Category::Conflict),
        (
            DriveError::Invalid(PRIVATE.into()),
            Reason::Invalid,
            Category::InvalidInput,
        ),
        (DriveError::Limit, Reason::Limit, Category::InvalidInput),
        (
            DriveError::Unavailable,
            Reason::Unavailable,
            Category::Dependency,
        ),
        (
            DriveError::OutcomeUnknown("requestID-secret".into()),
            Reason::OutcomeUnknown,
            Category::Unknown,
        ),
    ];
    for (source, reason, category) in cases {
        let original = source.clone();
        let failure = DriveClientFailure::new(Operation::Read, source);
        assert_eq!(failure.details().reason, reason);
        assert_eq!(failure.identity().domain, "workspace_drive_client");
        assert_eq!(failure.identity().details_version, 1);
        assert_eq!(failure.identity().category, category);
        assert!(std::ptr::eq(
            std::error::Error::source(&failure)
                .unwrap()
                .downcast_ref::<DriveError>()
                .unwrap(),
            failure.source_error(),
        ));
        match (&original, failure.source_error()) {
            (
                DriveError::Authorization {
                    operation,
                    resource,
                    denied,
                },
                DriveError::Authorization {
                    operation: actual_operation,
                    resource: actual_resource,
                    denied: actual_denied,
                },
            ) => {
                assert_eq!(operation, actual_operation);
                assert_eq!(resource, actual_resource);
                assert_eq!(denied, actual_denied);
            }
            (DriveError::Invalid(text), DriveError::Invalid(actual))
            | (DriveError::OutcomeUnknown(text), DriveError::OutcomeUnknown(actual)) => {
                assert_eq!(text, actual);
            }
            _ => assert_eq!(
                std::mem::discriminant(&original),
                std::mem::discriminant(failure.source_error()),
            ),
        }
        let mut facts = FailureFacts::new(failure).unwrap();
        let mut context =
            OperationContext::new("arbitrarycontext-secret", "receivedstage-secret").unwrap();
        context.operation_ref = Some("requestID-secret".into());
        facts.with_context(context).unwrap();
        facts
            .set_correlation(CorrelationRef::new("correlation-secret").unwrap())
            .unwrap();
        let browser = project_failure::<Browser, _>(&facts, &VISIBLE).unwrap();
        let expected = encode_projected_failure(&browser, budget()).unwrap();
        macro_rules! check_audience {
            ($audience:ty) => {{
                assert!(matches!(
                    project_failure::<$audience, _>(&facts, &Visibility::default()),
                    Err(ContractError::ProjectionUnavailable)
                ));
                let projected = project_failure::<$audience, _>(&facts, &VISIBLE).unwrap();
                let encoded = encode_projected_failure(&projected, budget()).unwrap();
                assert_eq!(encoded.as_bytes(), expected.as_bytes());
                let DecodeFailure::Known(received) =
                    decode_failure::<Details>(encoded.as_bytes(), budget())
                else {
                    panic!("known client cause must survive shared codec");
                };
                assert_eq!(received.facts().cause(), facts.cause().details());
                // A previous audience/visibility does not authorize a decoded value.
                assert!(matches!(
                    project_failure::<$audience, _>(received.facts(), &Visibility::default()),
                    Err(ContractError::ProjectionUnavailable)
                ));
            }};
        }
        check_audience!(Browser);
        check_audience!(Tool);
        check_audience!(InternalTransfer);
        check_audience!(Storage);
        check_audience!(Telemetry);
        let telemetry = project_failure::<Telemetry, _>(&facts, &VISIBLE).unwrap();
        let observed = observation_fields(&telemetry);
        assert_eq!(observed.domain, "workspace_drive_client");
        assert_eq!(observed.reason, reason.as_str());
        assert_eq!(observed.operation, Some("read"));
        assert_eq!(observed.stage, Some("operation"));
        assert_eq!(observed.correlation, None);
        assert_eq!(browser.context().operations.len(), 1);
        assert!(browser.context().operations[0].operation_ref.is_none());
        assert!(browser.context().causes.is_empty());
        assert!(browser.context().diagnostics.is_empty());
        let text = std::str::from_utf8(expected.as_bytes()).unwrap();
        assert!(text.contains("Drive client text read reported a failure"));
        for secret in [
            "credential-secret",
            "/private/",
            "provider-body",
            "HTTP status",
            "retry-safe",
            "requestID-secret",
            "workspaceID-secret",
            "arbitrarycontext-secret",
            "receivedstage-secret",
            "correlation-secret",
            "DriveRequest",
        ] {
            assert!(!text.contains(secret), "{text}");
        }
    }
}

#[test]
fn neither_invalid_prose_nor_request_identity_is_interpreted_as_a_detailed_cause() {
    for prose in [
        "deadline; decode failure; safely retry; not dispatched",
        "authorization denied; mutation committed; success",
        PRIVATE,
    ] {
        let invalid =
            DriveClientFailure::new(Operation::UpdateText, DriveError::Invalid(prose.into()));
        assert_eq!(invalid.details().reason, Reason::Invalid);
        let DriveError::Invalid(original) = invalid.into_source() else {
            panic!("original variant");
        };
        assert_eq!(original, prose);
        let unknown =
            DriveClientFailure::new(Operation::Read, DriveError::OutcomeUnknown(prose.into()));
        assert_eq!(unknown.details().reason, Reason::OutcomeUnknown);
        // Even a read can receive this existing variant: no new operation/result invariant.
        let DriveError::OutcomeUnknown(identity) = unknown.into_source() else {
            panic!("original identity");
        };
        assert_eq!(identity, prose);
    }
    let unavailable = DriveClientFailure::new(Operation::Bytes, DriveError::Unavailable);
    assert_eq!(unavailable.details().reason, Reason::Unavailable);
    assert!(matches!(unavailable.into_source(), DriveError::Unavailable));
}

#[test]
fn every_operation_has_a_closed_wire_kind_and_regenerated_operation_context() {
    for operation in [
        Operation::Workspace,
        Operation::Capabilities,
        Operation::Root,
        Operation::Metadata,
        Operation::List,
        Operation::Search,
        Operation::Read,
        Operation::Bytes,
        Operation::CreateFolder,
        Operation::CreateText,
        Operation::UpdateText,
        Operation::Relocate,
        Operation::Delete,
        Operation::Upload,
        Operation::RequestStatus,
    ] {
        assert_eq!(
            serde_json::to_value(operation).unwrap(),
            json!(operation.as_str()),
        );
        let facts =
            FailureFacts::new(DriveClientFailure::new(operation, DriveError::Unavailable)).unwrap();
        let projected = project_failure::<Browser, _>(&facts, &VISIBLE).unwrap();
        assert_eq!(projected.details().operation, operation);
        assert_eq!(
            projected.context().operations,
            vec![OperationContext::new(operation.as_str(), "operation").unwrap()],
        );
        assert!(
            projected
                .explanation()
                .contains("no more specific typed cause was retained")
        );
        assert!(!projected.explanation().contains("not dispatched"));
        assert!(!projected.explanation().contains("safe to retry"));
    }
}

#[test]
fn mutation_operation_selection_uses_only_the_existing_typed_variant() {
    let target = DriveEntryRef {
        workspace_id: PRIVATE.into(),
        node_id: "9007199254740993".into(),
    };
    let cases = [
        (
            DriveMutation::CreateFolder {
                parent: target.clone(),
                name: PRIVATE.into(),
            },
            Operation::CreateFolder,
        ),
        (
            DriveMutation::CreateText {
                parent: target.clone(),
                name: PRIVATE.into(),
                text: PRIVATE.into(),
                content_type: PRIVATE.into(),
            },
            Operation::CreateText,
        ),
        (
            DriveMutation::UpdateText {
                id: target.clone(),
                expected_mutation_id: PRIVATE.into(),
                text: PRIVATE.into(),
                content_type: PRIVATE.into(),
            },
            Operation::UpdateText,
        ),
        (
            DriveMutation::Relocate {
                id: target.clone(),
                expected_mutation_id: PRIVATE.into(),
                parent: target.clone(),
                name: PRIVATE.into(),
            },
            Operation::Relocate,
        ),
        (
            DriveMutation::Delete {
                id: target,
                expected_mutation_id: PRIVATE.into(),
            },
            Operation::Delete,
        ),
    ];
    for (mutation, expected) in cases {
        assert_eq!(Operation::from(&mutation), expected);
    }
}

#[test]
fn received_explanation_and_all_context_are_rebuilt_without_changing_typed_meaning() {
    let facts = FailureFacts::new(DriveClientFailure::new(
        Operation::UpdateText,
        DriveError::OutcomeUnknown("original-requestID-secret".into()),
    ))
    .unwrap();
    let stored = project_failure::<Storage, _>(&facts, &VISIBLE).unwrap();
    let stored = encode_projected_failure(&stored, budget()).unwrap();
    let original: Value = serde_json::from_slice(stored.as_bytes()).unwrap();
    // Contradictory text is an untrusted codec input, never recovery authority.
    for prose in [
        "SUCCESS; safely retry automatically",
        "NOT DISPATCHED; do not reconcile",
    ] {
        let mut wire = original.clone();
        wire["explanation"] = json!(prose);
        wire["context"] = json!({
            "operations": [{
                "operation": "untrustedoperation-secret", "stage": "not_dispatched",
                "operation_ref": "substitute-requestID-secret"
            }],
            "causes": [{"domain":"untrusted_domain", "reason":"deadline", "correlation":"causeID-secret"}],
            "diagnostics": [{"code":"safe_to_retry", "explanation":"provider-body-secret"}],
            "correlation":"correlation-secret", "supplementary_omitted":true
        });
        let DecodeFailure::Known(received) =
            decode_failure::<Details>(&serde_json::to_vec(&wire).unwrap(), budget())
        else {
            panic!("valid supported received facts");
        };
        assert_eq!(received.facts().cause().reason, Reason::OutcomeUnknown);
        assert_eq!(
            received.facts().cause().identity(),
            facts.cause().identity()
        );
        macro_rules! rebuilt {
            ($audience:ty) => {{
                let projected =
                    project_failure::<$audience, _>(received.facts(), &VISIBLE).unwrap();
                assert_eq!(
                    encode_projected_failure(&projected, budget())
                        .unwrap()
                        .as_bytes(),
                    stored.as_bytes(),
                );
            }};
        }
        rebuilt!(Browser);
        rebuilt!(Tool);
        rebuilt!(InternalTransfer);
        rebuilt!(Storage);
        rebuilt!(Telemetry);
        // Local original result remains the only typed error with request identity.
        assert!(matches!(
            facts.cause().source_error(),
            DriveError::OutcomeUnknown(id) if id == "original-requestID-secret"
        ));
    }
}

#[test]
fn shared_codec_rejects_injected_or_mismatched_details_and_does_not_conflate_storage_domain() {
    assert!(!Details::supports("drive", "conflict", 1));
    let facts = FailureFacts::new(DriveClientFailure::new(
        Operation::Metadata,
        DriveError::Conflict,
    ))
    .unwrap();
    let projected = project_failure::<Browser, _>(&facts, &VISIBLE).unwrap();
    assert!(matches!(
        encode_projected_failure(&projected, Budget::new(20, 20)),
        Err(ContractError::TooLarge)
    ));
    let encoded = encode_projected_failure(&projected, budget()).unwrap();
    let original: Value = serde_json::from_slice(encoded.as_bytes()).unwrap();
    for (field, value) in [
        ("domain", json!("drive")),
        ("reason", json!("deadline")),
        ("details_version", json!(2)),
    ] {
        let mut unsupported = original.clone();
        unsupported[field] = value;
        assert!(matches!(
            decode_failure::<Details>(&serde_json::to_vec(&unsupported).unwrap(), budget()),
            DecodeFailure::Unsupported(_)
        ));
    }
    for (field, value) in [("reason", json!("denied")), ("category", json!("internal"))] {
        let mut mismatch = original.clone();
        mismatch[field] = value;
        assert!(matches!(
            decode_failure::<Details>(&serde_json::to_vec(&mismatch).unwrap(), budget()),
            DecodeFailure::Corrupt(ContractError::InvalidDetails)
        ));
    }
    for (field, value) in [
        ("path", json!("/private/path")),
        ("provider", json!("provider-secret")),
        ("request_id", json!("requestID-secret")),
        ("resource", json!({"workspace_id":"secret"})),
        ("denied", json!(true)),
        ("retryable", json!(true)),
        ("state", json!("not_dispatched")),
        ("status", json!(503)),
    ] {
        let mut injected = original.clone();
        injected["details"][field] = value;
        assert!(matches!(
            decode_failure::<Details>(&serde_json::to_vec(&injected).unwrap(), budget()),
            DecodeFailure::Corrupt(ContractError::InvalidDetails)
        ));
    }
    for (field, value) in [
        ("operation", json!("arbitrary")),
        ("reason", json!("transport_deadline")),
    ] {
        let mut injected = original.clone();
        injected["details"][field] = value;
        assert!(matches!(
            decode_failure::<Details>(&serde_json::to_vec(&injected).unwrap(), budget()),
            DecodeFailure::Corrupt(ContractError::InvalidDetails)
        ));
    }
}
