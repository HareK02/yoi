//! HTTP adaptation of owner-selected control causes. Received text is never re-disclosed.
use api_macros::http::ApiBoundary;
use error_contract::problem::{HttpFailureStatus as S, ProblemClass};
use error_contract::{Browser, Budget, DecodeFailure, FailureFacts, project_failure};
pub use protocol::control_failure::RuntimeControlCause;

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
            | C::ExecutionRejected => S::Conflict,
            C::RuntimeStopped | C::ExecutionUnavailable | C::ConfigurationUnavailable => {
                S::Unavailable
            }
            C::ExecutionFailed
            | C::PersistenceFailed
            | C::StoreUnavailable
            | C::StateUnavailable
            | C::ClientSetupFailed => S::Internal,
            C::PersistenceOutcomeUnknown
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
        response: &api_macros::reqwest::ReceivedResponse,
    ) -> Result<Self, api_macros::reqwest::ClientFailure> {
        use api_macros::reqwest::ClientFailure;
        let status = response.status().as_u16();
        let media = response
            .content_type()?
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        if media.eq_ignore_ascii_case("application/problem+json") {
            return match response
                .decode_problem::<RuntimeControlCause>(&ControlBoundary, Budget::new(16384, 16384))
            {
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
        let mut error: Self = serde_json::from_slice(response.body())
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
}
