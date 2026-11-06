//! Runtime-owned HTTP classification for materialization/removal facts. The shared
//! codec validates transport metadata; this adapter never interprets received prose.
use api_macros::http::ApiBoundary;
use error_contract::problem::{HttpFailureStatus as Status, ProblemClass};
use error_contract::{Browser, Budget, FailureFacts, project_failure};
use workdir::management_failure::{WorkdirManagementFailure, WorkdirManagementVisibility};

pub struct ManagementBoundary;
impl ApiBoundary<WorkdirManagementFailure> for ManagementBoundary {
    fn classify(&self, cause: &WorkdirManagementFailure) -> ProblemClass {
        ProblemClass {
            status: match cause.reason.runtime_http_status() {
                400 => Status::BadRequest,
                403 => Status::Forbidden,
                404 => Status::NotFound,
                409 => Status::Conflict,
                502 => Status::BadGateway,
                503 => Status::Unavailable,
                _ => Status::Internal,
            },
            type_uri: "urn:yoi:problem:workdir-management",
            title: "Workdir management failed",
        }
    }
}
pub fn management_problem(
    cause: WorkdirManagementFailure,
) -> Result<api_macros::HttpErrorResponse, error_contract::ContractError> {
    let facts = FailureFacts::new(cause)?;
    let projected = project_failure::<Browser, _>(
        &facts,
        &WorkdirManagementVisibility {
            can_view_cause: true,
        },
    )?;
    api_macros::http::problem_response(
        &projected,
        &ManagementBoundary,
        "",
        Budget::new(16384, 16384),
    )
    .map(Into::into)
}

impl super::RuntimeApiError {
    pub(crate) fn decode_management_error(
        status: u16,
        content_type: &str,
        body: &[u8],
    ) -> Result<Self, api_macros::reqwest::ClientFailure> {
        use api_macros::reqwest::ClientFailure;
        use error_contract::{DecodeFailure, InternalTransfer};

        match api_macros::http::decode_problem_response::<WorkdirManagementFailure>(
            body,
            status,
            content_type,
            &ManagementBoundary,
            Budget::new(16384, 16384),
        ) {
            DecodeFailure::Known(failure) => {
                let cause = *failure.facts().cause();
                let projected = project_failure::<InternalTransfer, _>(
                    failure.facts(),
                    &WorkdirManagementVisibility {
                        can_view_cause: true,
                    },
                )
                .map_err(|_| ClientFailure::ErrorResponseDecode { status })?;
                Ok(
                    Self::new(status, cause.reason.as_str(), projected.explanation())
                        .with_management(cause),
                )
            }
            DecodeFailure::Unsupported(_) => {
                Err(ClientFailure::UnsupportedErrorResponse { status })
            }
            DecodeFailure::Corrupt(error_contract::ContractError::TooLarge) => {
                Err(ClientFailure::ResponseTooLarge { limit: 16384 })
            }
            DecodeFailure::Corrupt(_) => Err(ClientFailure::ErrorResponseDecode { status }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use error_contract::DecodeFailure;
    use workdir::management_failure::{WorkdirManagementOperation, WorkdirManagementReason};
    #[test]
    fn management_http_validates_transport_status_and_uses_local_explanation() {
        for reason in WorkdirManagementReason::ALL {
            let cause = WorkdirManagementFailure {
                operation: WorkdirManagementOperation::Cleanup,
                reason: *reason,
            };
            let response = management_problem(cause).unwrap();
            let DecodeFailure::Known(decoded) =
                api_macros::http::decode_problem_response::<WorkdirManagementFailure>(
                    &response.body,
                    response.status,
                    response.content_type,
                    &ManagementBoundary,
                    Budget::new(16384, 16384),
                )
            else {
                panic!("typed cause lost")
            };
            assert_eq!(decoded.facts().cause(), &cause);
            assert!(matches!(
                api_macros::http::decode_problem_response::<WorkdirManagementFailure>(
                    &response.body,
                    418,
                    response.content_type,
                    &ManagementBoundary,
                    Budget::new(16384, 16384)
                ),
                DecodeFailure::Corrupt(_)
            ));
        }
    }
}
