//! Workdir HTTP projection keeps the operation cause and audit receipts independent.
use api_macros::{HttpError, HttpErrorResponse};
use error_contract::problem::{ApiBoundary, HttpFailureStatus, ProblemClass, ProblemDetails};
use error_contract::{Browser, Budget, DecodeFailure, ProjectedFailure};
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use workdir::failure::WorkdirFailureDetails;

use crate::WorkdirOperationApiError;

pub struct WorkdirHttpApiError {
    legacy: WorkdirOperationApiError,
    details: Option<WorkdirFailureDetails>,
    projected: Option<Vec<u8>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_open_round_trip_keeps_gateway_status_separate_from_provider_classification() {
        use workdir::failure::{WorkdirFailureOperation, WorkdirFailureVisibility};
        use workdir::http::WorkdirTransportErrorCode as Code;
        for classification in [Code::Unavailable, Code::Denied, Code::OutcomeUnknown] {
            let details = WorkdirFailureDetails {
                operation: WorkdirFailureOperation::SessionOpen,
                classification,
                denial_reason: None,
                failure_reason: Some(workdir::WorkdirFailureReason::ProviderResponseLost),
            };
            let facts = error_contract::FailureFacts::new(details).unwrap();
            let projected = error_contract::project_failure::<Browser, _>(
                &facts,
                &WorkdirFailureVisibility {
                    can_view_cause: true,
                },
            )
            .unwrap();
            let legacy = WorkdirOperationApiError::api(crate::RepositoryApiError::new(
                502,
                "Bad Gateway",
                "provider-secret /private/path",
                Vec::new(),
            ));
            let error = WorkdirHttpApiError::projected(legacy, projected);
            let response = error.custom_response().unwrap();
            assert_eq!(error.status_code(), 502);
            assert_eq!(response.status, 502);
            let wire = serde_json::to_vec(&error).unwrap();
            assert!(!String::from_utf8_lossy(&wire).contains("provider-secret"));
            let decoded =
                WorkdirHttpApiError::decode_received(502, response.content_type, &wire).unwrap();
            assert_eq!(decoded.status_code(), 502);
            assert_eq!(decoded.details(), Some(&details));
            assert!(
                WorkdirHttpApiError::decode_received(
                    classification.http_status(),
                    response.content_type,
                    &wire,
                )
                .is_err()
            );
        }
    }

    #[test]
    fn typed_problem_schema_retains_domain_details() {
        let schema = serde_json::to_value(schemars::schema_for!(WorkdirHttpProblem)).unwrap();
        let reference = schema["properties"]["yoi_failure"]["$ref"]
            .as_str()
            .unwrap();
        let name = reference.strip_prefix("#/$defs/").unwrap();
        assert_eq!(
            schema["$defs"][name]["properties"]["details"]["$ref"],
            "#/$defs/WorkdirFailureDetails"
        );
    }
}

pub struct WorkdirHttpBoundary;
impl ApiBoundary<WorkdirFailureDetails> for WorkdirHttpBoundary {
    fn classify(&self, details: &WorkdirFailureDetails) -> ProblemClass {
        use workdir::http::WorkdirTransportErrorCode::*;
        // This endpoint reports a failed prerequisite open as 502, independently
        // of the provider's classification (for example Unavailable or Denied).
        if details.operation == workdir::failure::WorkdirFailureOperation::SessionOpen {
            return ProblemClass {
                status: HttpFailureStatus::BadGateway,
                type_uri: "urn:yoi:problem:workdir",
                title: "Workdir session opening failed",
            };
        }
        ProblemClass {
            status: match details.classification {
                InvalidRequest
                | Unsupported
                | BrokenSymlink
                | SymlinkTargetIsDirectory
                | IsDirectory
                | SymlinkDirectoryNotTraversed => HttpFailureStatus::BadRequest,
                Denied | OutOfScope | SymlinkOutOfScope | ReadOnly => HttpFailureStatus::Forbidden,
                NotFound | UnknownCommand => HttpFailureStatus::NotFound,
                Conflict => HttpFailureStatus::Conflict,
                Unavailable | NotDispatched => HttpFailureStatus::Unavailable,
                Transport => HttpFailureStatus::BadGateway,
                Io | Internal | OutcomeUnknown => HttpFailureStatus::Internal,
            },
            type_uri: "urn:yoi:problem:workdir",
            title: "Workdir operation failed",
        }
    }
}
fn budget() -> Budget {
    Budget::new(
        error_contract::MAX_FAILURE_BYTES,
        error_contract::MAX_FAILURE_BYTES,
    )
}

// RFC extensions carry the existing domain result, not a new cause or retry state.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct WorkdirHttpProblem {
    #[serde(flatten)]
    problem: ProblemDetails<WorkdirFailureDetails>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    audit_failures: Vec<workdir::http::WorkdirAuditFailure>,
}
impl WorkdirHttpApiError {
    pub fn projected(
        mut legacy: WorkdirOperationApiError,
        projected: ProjectedFailure<Browser, WorkdirFailureDetails>,
    ) -> Self {
        let details = *projected.details();
        let response =
            api_macros::http::problem_response(&projected, &WorkdirHttpBoundary, "", budget())
                .expect("fixed boundary budget accommodates the static fallback");
        let problem: ProblemDetails<WorkdirFailureDetails> =
            serde_json::from_slice(response.body())
                .expect("shared encoder produced a Problem Details document");
        let body = serde_json::to_vec(&WorkdirHttpProblem {
            problem,
            audit_failures: legacy.audit_failures.clone(),
        })
        .expect("closed Workdir response is serializable");
        // Never truncate an audit receipt or silently replace an original result.
        // An oversized extension retains the established domain response instead.
        if body.len() > error_contract::MAX_FAILURE_BYTES {
            legacy.message = projected.explanation().into();
            legacy.diagnostics = None;
            return legacy.into();
        }
        Self {
            legacy,
            details: Some(details),
            projected: Some(body),
        }
    }
    pub fn details(&self) -> Option<&WorkdirFailureDetails> {
        self.details.as_ref()
    }
    pub fn legacy(&self) -> &WorkdirOperationApiError {
        &self.legacy
    }

    pub fn decode_received(
        status: u16,
        content_type: &str,
        body: &[u8],
    ) -> Result<Self, api_macros::reqwest::ClientFailure> {
        use api_macros::reqwest::ClientFailure;
        let media = content_type.split(';').next().unwrap_or("").trim();
        if media.eq_ignore_ascii_case(error_contract::problem::PROBLEM_CONTENT_TYPE) {
            return match api_macros::http::decode_problem_response::<WorkdirFailureDetails>(
                body,
                status,
                content_type,
                &WorkdirHttpBoundary,
                budget(),
            ) {
                DecodeFailure::Known(received) => {
                    let details = *received.facts().cause();
                    let wire: WorkdirHttpProblem = serde_json::from_slice(body)
                        .map_err(|_| ClientFailure::ErrorResponseDecode { status })?;
                    let mut legacy = if details.operation
                        == workdir::failure::WorkdirFailureOperation::SessionOpen
                    {
                        let mut legacy =
                            WorkdirOperationApiError::api(crate::RepositoryApiError::new(
                                status,
                                "Bad Gateway",
                                "Workdir session opening failed",
                                Vec::new(),
                            ));
                        legacy.denial_reason = details.denial_reason;
                        legacy.failure_reason = details.failure_reason;
                        legacy.failure_details = Some(details);
                        legacy
                    } else {
                        WorkdirOperationApiError::provider(workdir::http::WorkdirTransportError {
                            code: details.classification,
                            message: "Workdir operation failed".into(),
                            denial_reason: details.denial_reason,
                            failure_reason: details.failure_reason,
                        })
                    };
                    legacy.audit_failures = wire.audit_failures;
                    Ok(Self {
                        legacy,
                        details: Some(details),
                        projected: None,
                    })
                }
                DecodeFailure::Unsupported(_) => {
                    Err(ClientFailure::UnsupportedErrorResponse { status })
                }
                DecodeFailure::Corrupt(_) => Err(ClientFailure::ErrorResponseDecode { status }),
            };
        }
        if !media.eq_ignore_ascii_case("application/json")
            || body.len() > error_contract::MAX_FAILURE_BYTES
        {
            return Err(ClientFailure::ErrorResponseDecode { status });
        }
        let legacy = match serde_json::from_slice::<workdir::http::WorkdirTransportError>(body) {
            Ok(transport) => {
                #[derive(Deserialize)]
                struct Receipts {
                    #[serde(default)]
                    audit_failures: Vec<workdir::http::WorkdirAuditFailure>,
                }
                let receipts: Receipts = serde_json::from_slice(body)
                    .map_err(|_| ClientFailure::ErrorResponseDecode { status })?;
                let mut legacy = WorkdirOperationApiError::provider(transport);
                legacy.audit_failures = receipts.audit_failures;
                legacy
            }
            Err(_) => serde_json::from_slice::<WorkdirOperationApiError>(body)
                .map_err(|_| ClientFailure::ErrorResponseDecode { status })?,
        };
        if let Some(code) = legacy.code {
            if code.http_status() != status {
                return Err(ClientFailure::ErrorResponseDecode { status });
            }
        }
        Ok(legacy.into())
    }
}
impl From<WorkdirOperationApiError> for WorkdirHttpApiError {
    fn from(legacy: WorkdirOperationApiError) -> Self {
        Self {
            legacy,
            details: None,
            projected: None,
        }
    }
}
impl std::fmt::Debug for WorkdirHttpApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkdirHttpApiError")
            .field("details", &self.details)
            .finish_non_exhaustive()
    }
}
impl HttpError for WorkdirHttpApiError {
    fn status_code(&self) -> u16 {
        self.legacy.status_code()
    }
    fn custom_response(&self) -> Option<HttpErrorResponse> {
        self.projected.as_ref().map(|body| HttpErrorResponse {
            status: self.status_code(),
            content_type: error_contract::problem::PROBLEM_CONTENT_TYPE,
            body: body.clone().into(),
        })
    }
}
impl api_macros::HttpRequestError for WorkdirHttpApiError {
    fn from_request_rejection(status: u16, message: String) -> Self {
        WorkdirOperationApiError::from_request_rejection(status, message).into()
    }
}
impl Serialize for WorkdirHttpApiError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match &self.projected {
            Some(body) => serde_json::from_slice::<WorkdirHttpProblem>(body)
                .map_err(serde::ser::Error::custom)?
                .serialize(serializer),
            None => self.legacy.serialize(serializer),
        }
    }
}
impl<'de> Deserialize<'de> for WorkdirHttpApiError {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        WorkdirOperationApiError::deserialize(deserializer).map(Into::into)
    }
}
#[allow(dead_code)]
#[derive(JsonSchema)]
#[serde(untagged)]
enum WorkdirHttpErrorWire {
    Problem(WorkdirHttpProblem),
    Legacy(WorkdirOperationApiError),
}
impl JsonSchema for WorkdirHttpApiError {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "WorkdirHttpApiError".into()
    }
    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        WorkdirHttpErrorWire::json_schema(generator)
    }
}
impl api_macros::openapi::OpenApiSchema for WorkdirHttpApiError {}
impl api_macros::reqwest::HttpClientError for WorkdirHttpApiError {
    fn decode_error(
        response: &api_macros::reqwest::ReceivedResponse,
    ) -> Result<Self, api_macros::reqwest::ClientFailure> {
        Self::decode_received(
            response.status().as_u16(),
            response.content_type()?,
            response.body(),
        )
    }
}
