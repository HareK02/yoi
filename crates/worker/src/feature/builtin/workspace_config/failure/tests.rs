use super::*;
use error_contract::{
    Audience, Budget, DecodeFailure, FailureContext, decode_failure, encode_projected_failure,
    project_failure,
};
use wip_protocol::{ProtocolError, ProtocolErrorCode};

fn projection_for<A: Audience>(details: ConfigFailureDetails)
where
    ConfigFailureDetails:
        ProjectFor<A, Authorization = ConfigFailureVisibility, Details = ConfigFailureDetails>,
{
    let facts = FailureFacts::new(details).unwrap();
    assert!(matches!(
        project_failure::<A, _>(&facts, &ConfigFailureVisibility::default()),
        Err(ContractError::ProjectionUnavailable)
    ));
    let visible = ConfigFailureVisibility {
        can_view_cause: true,
    };
    let projected = project_failure::<A, _>(&facts, &visible).unwrap();
    assert_eq!(*projected.details(), details);
    assert_eq!(projected.identity().domain, "workspace_config_client");
    assert_eq!(*projected.context(), FailureContext::default());
    let original_explanation = projected.explanation().to_owned();
    let encoded = encode_projected_failure(&projected, Budget::new(4096, 4096)).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(encoded.as_bytes()).unwrap();
    value["explanation"] = "secret-provider-prose /private/config HTTP status 403".into();
    value["context"] = serde_json::json!({
        "operations":[{"operation":"secret-operation", "stage":"secret-stage", "operation_ref":"secret-request"}],
        "causes":[{"domain":"secret-domain", "reason":"secret-reason", "correlation":"secret-correlation"}],
        "diagnostics":[{"code":"secret-diagnostic", "explanation":"secret-provider-prose"}],
        "correlation":"secret-correlation", "supplementary_omitted":true
    });
    let received = match decode_failure::<ConfigFailureDetails>(
        &serde_json::to_vec(&value).unwrap(),
        Budget::new(4096, 4096),
    ) {
        DecodeFailure::Known(received) => received,
        _ => panic!("supported envelope rejected"),
    };
    // A prior disclosure is not visibility for this read, even for Storage/Telemetry.
    assert!(matches!(
        project_failure::<A, _>(received.facts(), &ConfigFailureVisibility::default()),
        Err(ContractError::ProjectionUnavailable)
    ));
    let fresh = project_failure::<A, _>(received.facts(), &visible).unwrap();
    assert_eq!(*fresh.details(), details);
    assert_eq!(fresh.explanation(), original_explanation);
    assert_eq!(*fresh.context(), FailureContext::default());
    let output = encode_projected_failure(&fresh, Budget::new(4096, 4096)).unwrap();
    let text = std::str::from_utf8(output.as_bytes()).unwrap();
    for secret in [
        "secret",
        "/private/config",
        "HTTP status",
        "PermissionResource",
    ] {
        assert!(
            !text.contains(secret),
            "projection leaked received/source context"
        );
    }
    // Wire details are a closed allowlist, not a place to hide old provider fields.
    value["details"]["path"] = "/private/config".into();
    assert!(matches!(
        decode_failure::<ConfigFailureDetails>(
            &serde_json::to_vec(&value).unwrap(),
            Budget::new(4096, 4096),
        ),
        DecodeFailure::Corrupt(_)
    ));
}

/// Shared by the real backend fixtures so disclosure tests start from production causes.
pub(crate) fn assert_projection(details: ConfigFailureDetails) {
    projection_for::<Browser>(details);
    projection_for::<Tool>(details);
    projection_for::<InternalTransfer>(details);
    projection_for::<Storage>(details);
    projection_for::<Telemetry>(details);
}

#[test]
fn local_source_and_changed_prose_do_not_change_outcome_or_projection() {
    for message in [
        "first wording",
        "secret-provider-prose HTTP status 403 /private/config",
    ] {
        let failure = ConfigFailure::new(
            ConfigFailureOperation::Commit,
            ConfigFailureStage::Response,
            ConfigFailureReason::Decode,
            WipOperationError::OutcomeUnknown(message.into()),
        );
        assert_projection(*failure.details());
        let facts = FailureFacts::new(failure).unwrap();
        assert!(matches!(
            project_failure::<Tool, _>(&facts, &ConfigFailureVisibility::default()),
            Err(ContractError::ProjectionUnavailable)
        ));
        let projected = project_failure::<Tool, _>(
            &facts,
            &ConfigFailureVisibility {
                can_view_cause: true,
            },
        )
        .unwrap();
        assert!(!projected.explanation().contains(message));
        assert!(
            matches!(facts.cause().source_error(), WipOperationError::OutcomeUnknown(text) if text == message)
        );
        assert!(!format!("{:?}", facts.cause()).contains(message));
    }
    let failure = ConfigFailure::new(
        ConfigFailureOperation::Read,
        ConfigFailureStage::Response,
        ConfigFailureReason::BackendRejected,
        WipOperationError::Protocol(ProtocolError {
            code: ProtocolErrorCode::ValidatorMismatch,
            message: "secret-source".into(),
        }),
    );
    assert!(
        matches!(failure.source_error(), WipOperationError::Protocol(error) if error.code == ProtocolErrorCode::ValidatorMismatch)
    );
    assert!(
        matches!(failure.into_source(), WipOperationError::Protocol(error) if error.code == ProtocolErrorCode::ValidatorMismatch && error.message == "secret-source")
    );
}
