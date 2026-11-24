//! HTTP adaptation of owner-selected control causes. Received text is never re-disclosed.
use api_macros::http::ApiBoundary;
use error_contract::problem::{HttpFailureStatus as S, ProblemClass};
use error_contract::{Browser, Budget, DecodeFailure, FailureFacts, project_failure};
pub use protocol::control_failure::{RuntimeControlCause, RuntimeControlFailure};

pub struct ControlBoundary;
impl ApiBoundary<RuntimeControlCause> for ControlBoundary {
    fn classify(&self, cause: &RuntimeControlCause) -> ProblemClass {
        use RuntimeControlCause as C;
        let status = match cause {
            C::WorkerMissing | C::RuntimeMissing => S::NotFound,
            C::InvalidWorkerIdentifier
            | C::InvalidRequest
            | C::RequestEncodingFailed
            | C::UnsupportedOperation => S::BadRequest,
            C::AuthorizationFailed | C::WorkspaceBindingMismatch => S::Forbidden,
            C::CreateRequestConflict
            | C::RestoreObservationStale
            | C::RestoreObservationConflict
            | C::RestoreAdmissionBusy
            | C::StopObservationConflict
            | C::StopRequestConflict
            | C::StopAdmissionBusy
            | C::StopRequestInterrupted
            | C::ExecutionRejected => S::Conflict,
            C::RuntimeStopped | C::ExecutionUnavailable | C::ConfigurationUnavailable => {
                S::Unavailable
            }
            C::ExecutionFailed
            | C::PersistenceFailed
            | C::StopTerminatedPersistenceFailed
            | C::StoreUnavailable
            | C::StateUnavailable
            | C::ClientSetupFailed => S::Internal,
            C::PersistenceOutcomeUnknown
            | C::StopTerminatedPersistenceOutcomeUnknown
            | C::StopTerminatedCleanupFailed
            | C::TransportFailed
            | C::ResultUnavailable
            | C::RecordingSourceUnavailable
            | C::RecordingOwnershipUnknown
            | C::RecordingExecutionUnconfirmed => S::Unavailable,
            C::TimedOut => S::GatewayTimeout,
            C::WorkingDirectoryFailed
            | C::ConnectionFailed
            | C::ResponseTooLarge
            | C::InvalidResponse
            | C::RemoteRejected => S::BadGateway,
        };
        ProblemClass {
            status,
            type_uri: "urn:yoi:problem:runtime-control",
            title: "Runtime control failed",
        }
    }
}

impl ApiBoundary<RuntimeControlFailure> for ControlBoundary {
    fn classify(&self, failure: &RuntimeControlFailure) -> ProblemClass {
        <Self as ApiBoundary<RuntimeControlCause>>::classify(self, &failure.cause)
    }
}

pub fn control_problem_with_command(
    cause: RuntimeControlCause,
    command: Option<protocol::WorkerCommandResult>,
) -> Result<api_macros::HttpErrorResponse, error_contract::ContractError> {
    let Some(command) = command else {
        return control_problem(cause);
    };
    let facts = FailureFacts::new(RuntimeControlFailure {
        cause,
        command: Some(command),
    })?;
    let projected = project_failure::<Browser, _>(&facts, &())?;
    api_macros::http::problem_response(&projected, &ControlBoundary, "", Budget::new(16384, 16384))
        .map(Into::into)
}

pub fn control_explanation(
    cause: RuntimeControlCause,
) -> Result<String, error_contract::ContractError> {
    let projected = project_failure::<Browser, _>(&FailureFacts::new(cause)?, &())?;
    Ok(projected.explanation().to_owned())
}

pub fn control_problem(
    cause: RuntimeControlCause,
) -> Result<api_macros::HttpErrorResponse, error_contract::ContractError> {
    let projected = project_failure::<Browser, _>(&FailureFacts::new(cause)?, &())?;
    api_macros::http::problem_response(&projected, &ControlBoundary, "", Budget::new(16384, 16384))
        .map(Into::into)
}

impl super::RuntimeApiError {
    pub(crate) fn decode_control_error(
        status: u16,
        content_type: &str,
        body: &[u8],
    ) -> Result<Self, api_macros::reqwest::ClientFailure> {
        use api_macros::reqwest::ClientFailure;
        let media = content_type.split(';').next().unwrap_or("").trim();
        if media.eq_ignore_ascii_case("application/problem+json") {
            // New details retain command evidence; cause-only responses remain readable.
            if let DecodeFailure::Known(failure) =
                api_macros::http::decode_problem_response::<RuntimeControlFailure>(
                    body,
                    status,
                    content_type,
                    &ControlBoundary,
                    Budget::new(16384, 16384),
                )
            {
                let details = failure.facts().cause();
                return Ok(
                    Self::new(status, details.cause.reason(), details.cause.explanation())
                        .with_control(details.cause)
                        .with_command(details.command.clone()),
                );
            }
            return match api_macros::http::decode_problem_response::<RuntimeControlCause>(
                body,
                status,
                content_type,
                &ControlBoundary,
                Budget::new(16384, 16384),
            ) {
                DecodeFailure::Known(failure) => {
                    let cause = *failure.facts().cause();
                    Ok(Self::new(status, cause.reason(), cause.explanation()).with_control(cause))
                }
                DecodeFailure::Unsupported(_) => {
                    Err(ClientFailure::UnsupportedErrorResponse { status })
                }
                DecodeFailure::Corrupt(error_contract::ContractError::TooLarge) => {
                    Err(ClientFailure::ResponseTooLarge { limit: 16384 })
                }
                DecodeFailure::Corrupt(_) => Err(ClientFailure::ErrorResponseDecode { status }),
            };
        }
        // Reader-first compatibility for Runtime startup and old non-control endpoints.
        // The absence of a typed cause remains absence; do not parse legacy code/prose.
        let mut error: Self = serde_json::from_slice(body)
            .map_err(|_| ClientFailure::ErrorResponseDecode { status })?;
        error.status = status;
        Ok(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_control_problems_roundtrip_with_transport_metadata_and_reject_mismatched_status() {
        for cause in RuntimeControlCause::ALL {
            let response = control_problem(*cause).unwrap();
            let result = api_macros::http::decode_problem_response::<RuntimeControlCause>(
                &response.body,
                response.status,
                response.content_type,
                &ControlBoundary,
                Budget::new(16384, 16384),
            );
            match result {
                DecodeFailure::Known(value) => assert_eq!(value.facts().cause(), cause),
                _ => panic!("control problem not decoded"),
            }
            assert!(matches!(
                api_macros::http::decode_problem_response::<RuntimeControlCause>(
                    &response.body,
                    418,
                    response.content_type,
                    &ControlBoundary,
                    Budget::new(16384, 16384)
                ),
                DecodeFailure::Corrupt(_)
            ));
        }
    }
    #[test]
    fn command_http_failure_retains_unknown_delivery_and_rejection_distinctly() {
        use protocol::{WorkerCommandKind as K, WorkerCommandResult as R};
        let acknowledged = |disposition| R::Acknowledged {
            acknowledgement: protocol::WorkerCommandAcknowledgement {
                command_id: 42,
                command: K::Cancel,
                disposition,
                state: protocol::WorkerStateSnapshot::initial(),
            },
        };
        for (cause, evidence) in [
            (
                RuntimeControlCause::ResultUnavailable,
                R::Unconfirmed {
                    command: K::Cancel,
                    command_id: Some(42),
                    delivery: protocol::WorkerCommandDelivery::Enqueued,
                    reason: protocol::WorkerCommandObservationFailure::TimedOut,
                },
            ),
            (
                RuntimeControlCause::ExecutionRejected,
                acknowledged(protocol::WorkerCommandDisposition::Conflict),
            ),
            (
                RuntimeControlCause::ExecutionRejected,
                acknowledged(protocol::WorkerCommandDisposition::StaleCommandId),
            ),
            (
                RuntimeControlCause::ExecutionRejected,
                acknowledged(protocol::WorkerCommandDisposition::InvalidState),
            ),
        ] {
            let response = control_problem_with_command(cause, Some(evidence.clone())).unwrap();
            let received = super::super::RuntimeApiError::decode_control_error(
                response.status,
                response.content_type,
                &response.body,
            )
            .unwrap();
            assert_eq!(received.control_cause(), Some(cause));
            assert_eq!(received.command_result(), Some(&evidence));
            let reprojected = api_macros::HttpError::custom_response(&received).unwrap();
            let decoded = api_macros::http::decode_problem_response::<RuntimeControlFailure>(
                &reprojected.body,
                reprojected.status,
                reprojected.content_type,
                &ControlBoundary,
                Budget::new(16384, 16384),
            );
            match decoded {
                DecodeFailure::Known(failure) => {
                    assert_eq!(failure.facts().cause().command, Some(evidence))
                }
                _ => panic!("command evidence lost after redisclosure"),
            }
        }
    }

    #[test]
    fn interaction_and_lifecycle_wire_preserve_admission_snapshot_without_completion() {
        let command = serde_json::json!({
            "kind":"acknowledged", "acknowledgement": {
                "command_id":42, "command":"compact", "disposition":"accepted",
                "state": serde_json::to_value(protocol::WorkerStateSnapshot::initial()).unwrap()
            }
        });
        let value = serde_json::json!({"worker_ref":{"worker_id":"01968f08-2122-7000-8000-000000000001"}, "status":"idle", "lifecycle":"active", "command":command});
        let lifecycle: super::super::WorkerLifecycleAck =
            serde_json::from_value(value.clone()).unwrap();
        let interaction: super::super::WorkerInteractionAck =
            serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(lifecycle).unwrap()["command"], command);
        assert_eq!(
            serde_json::to_value(interaction).unwrap()["command"],
            command
        );
        let mut missing = value;
        missing.as_object_mut().unwrap().remove("command");
        assert!(
            serde_json::from_value::<super::super::WorkerLifecycleAck>(missing.clone())
                .unwrap()
                .command
                .is_none()
        );
        assert!(
            serde_json::from_value::<super::super::WorkerInteractionAck>(missing)
                .unwrap()
                .command
                .is_none()
        );
    }
}
