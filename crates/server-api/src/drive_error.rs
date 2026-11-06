//! Drive's strict legacy DTO stays separate from HTTP projection and received facts.
use api_macros::{HttpError, HttpErrorResponse};
use error_contract::problem::{ApiBoundary, HttpFailureStatus, ProblemClass, ProblemDetails};
use error_contract::{Browser, Budget, DecodeFailure, ProjectedFailure};
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use workspace_drive::failure::DriveFailure;

use crate::{DriveApiError, DriveApiErrorCode, DriveHttpFailureDetails};

pub struct DriveHttpApiError {
    legacy: DriveApiError,
    details: Option<DriveHttpFailureDetails>,
    projected: Option<error_contract::problem::ProblemResponse>,
    source: Option<DriveFailure>,
}

pub struct DriveHttpBoundary;
impl ApiBoundary<DriveHttpFailureDetails> for DriveHttpBoundary {
    fn classify(&self, details: &DriveHttpFailureDetails) -> ProblemClass {
        use DriveApiErrorCode::*;
        ProblemClass {
            status: match details.code {
                Denied => HttpFailureStatus::Forbidden,
                NotFound => HttpFailureStatus::NotFound,
                Conflict => HttpFailureStatus::Conflict,
                Invalid => HttpFailureStatus::BadRequest,
                Limit => HttpFailureStatus::PayloadTooLarge,
                StorageUnavailable | OutcomeUnknown => HttpFailureStatus::Unavailable,
            },
            type_uri: "urn:yoi:problem:drive",
            title: "Drive operation failed",
        }
    }
}
fn budget() -> Budget {
    Budget::new(
        error_contract::MAX_FAILURE_BYTES,
        error_contract::MAX_FAILURE_BYTES,
    )
}
impl DriveHttpApiError {
    pub fn new(code: DriveApiErrorCode) -> Self {
        DriveApiError::new(code).into()
    }
    /// Only a currently authorized Browser projection creates the HTTP encoder capability.
    pub fn projected(projected: ProjectedFailure<Browser, DriveHttpFailureDetails>) -> Self {
        let details = *projected.details();
        let response =
            api_macros::http::problem_response(&projected, &DriveHttpBoundary, "", budget())
                .expect("fixed boundary budget accommodates the static fallback");
        let mut legacy = DriveApiError::new(details.code);
        legacy.classification = details.classification;
        Self {
            legacy,
            details: Some(details),
            projected: Some(response),
            source: None,
        }
    }
    pub fn with_projection(
        self,
        projected: ProjectedFailure<Browser, DriveHttpFailureDetails>,
    ) -> Self {
        let mut error = Self::projected(projected);
        error.source = self.source;
        error
    }
    /// The source is local diagnostic evidence and is never serialized.
    pub fn with_source(mut self, source: DriveFailure) -> Self {
        self.source = Some(source);
        self
    }
    pub fn source_failure(&self) -> Option<&DriveFailure> {
        self.source.as_ref()
    }
    pub fn details(&self) -> Option<&DriveHttpFailureDetails> {
        self.details.as_ref()
    }
    pub fn legacy(&self) -> &DriveApiError {
        &self.legacy
    }
}
impl From<DriveApiError> for DriveHttpApiError {
    fn from(legacy: DriveApiError) -> Self {
        Self {
            legacy,
            details: None,
            projected: None,
            source: None,
        }
    }
}
impl std::ops::Deref for DriveHttpApiError {
    type Target = DriveApiError;
    fn deref(&self) -> &Self::Target {
        &self.legacy
    }
}
impl std::fmt::Debug for DriveHttpApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DriveHttpApiError")
            .field("legacy", &self.legacy)
            .field("details", &self.details)
            .finish_non_exhaustive()
    }
}
impl std::fmt::Display for DriveHttpApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.legacy.message)
    }
}
impl std::error::Error for DriveHttpApiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_ref().map(|source| source as _)
    }
}
impl HttpError for DriveHttpApiError {
    fn status_code(&self) -> u16 {
        self.projected
            .as_ref()
            .map_or_else(|| self.legacy.status_code(), |p| p.status)
    }
    fn custom_response(&self) -> Option<HttpErrorResponse> {
        self.projected.as_ref().map(|p| HttpErrorResponse {
            status: p.status,
            content_type: p.content_type,
            body: p.body().to_vec().into(),
        })
    }
}
impl api_macros::HttpRequestError for DriveHttpApiError {
    fn from_request_rejection(status: u16, _message: String) -> Self {
        use crate::{DriveHttpFailureVisibility, DriveHttpOperation};
        let legacy = DriveApiError::new(if status == 413 {
            DriveApiErrorCode::Limit
        } else {
            DriveApiErrorCode::Invalid
        });
        let facts = error_contract::FailureFacts::new(DriveHttpFailureDetails::result(
            DriveHttpOperation::Request,
            &legacy,
        ))
        .expect("closed result facts");
        // The rejection itself is public; it reveals no storage or authorization evidence.
        Self::projected(
            error_contract::project_failure::<Browser, _>(
                &facts,
                &DriveHttpFailureVisibility {
                    can_view_result: true,
                    can_view_storage_cause: false,
                },
            )
            .expect("static request rejection projection"),
        )
    }
}
impl Serialize for DriveHttpApiError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match &self.projected {
            Some(p) => serde_json::from_slice::<ProblemDetails<DriveHttpFailureDetails>>(p.body())
                .map_err(serde::ser::Error::custom)?
                .serialize(serializer),
            None => self.legacy.serialize(serializer),
        }
    }
}
impl<'de> Deserialize<'de> for DriveHttpApiError {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        DriveApiError::deserialize(deserializer).map(Into::into)
    }
}
#[allow(dead_code)]
#[derive(JsonSchema)]
#[serde(untagged)]
enum DriveHttpErrorWire {
    Problem(ProblemDetails<DriveHttpFailureDetails>),
    Legacy(DriveApiError),
}
impl JsonSchema for DriveHttpApiError {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "DriveHttpApiError".into()
    }
    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        DriveHttpErrorWire::json_schema(generator)
    }
}
impl api_macros::openapi::OpenApiSchema for DriveHttpApiError {
    fn response_schema(
        builder: &mut api_macros::openapi::OpenApiBuilder,
        content_type: &str,
    ) -> Result<serde_json::Value, api_macros::openapi::OpenApiError> {
        match content_type {
            "application/json" => builder.schema_ref::<DriveApiError>(),
            error_contract::problem::PROBLEM_CONTENT_TYPE => {
                builder.schema_ref::<DriveHttpProblem>()
            }
            _ => Err(api_macros::openapi::OpenApiError::InvalidContract(
                "unsupported Drive error representation".into(),
            )),
        }
    }
}
impl api_macros::reqwest::HttpClientError for DriveHttpApiError {
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
impl DriveHttpApiError {
    /// Decode bounded transport bytes through the same contract as the generated client.
    /// Received facts are not a disclosure capability; callers must reproject them.
    pub fn decode_received(
        status: u16,
        content_type: &str,
        body: &[u8],
    ) -> Result<Self, api_macros::reqwest::ClientFailure> {
        use api_macros::reqwest::ClientFailure;
        let media = content_type.split(';').next().unwrap_or("").trim();
        if media.eq_ignore_ascii_case(error_contract::problem::PROBLEM_CONTENT_TYPE) {
            return match api_macros::http::decode_problem_response::<DriveHttpFailureDetails>(
                body,
                status,
                content_type,
                &DriveHttpBoundary,
                budget(),
            ) {
                DecodeFailure::Known(received) => {
                    let details = *received.facts().cause();
                    let mut legacy = DriveApiError::new(details.code);
                    legacy.classification = details.classification;
                    Ok(Self {
                        legacy,
                        details: Some(details),
                        projected: None,
                        source: None,
                    })
                }
                DecodeFailure::Unsupported(_) => {
                    Err(ClientFailure::UnsupportedErrorResponse { status })
                }
                DecodeFailure::Corrupt(_) => Err(ClientFailure::ErrorResponseDecode { status }),
            };
        }
        if !media.eq_ignore_ascii_case("application/json") {
            return Err(ClientFailure::UnsupportedErrorResponse { status });
        }
        let legacy: DriveApiError = serde_json::from_slice(body)
            .map_err(|_| ClientFailure::ErrorResponseDecode { status })?;
        if legacy.status_code() != status {
            return Err(ClientFailure::ErrorResponseDecode { status });
        }
        Ok(legacy.into())
    }
}
#[derive(JsonSchema)]
#[serde(transparent)]
struct DriveHttpProblem(ProblemDetails<DriveHttpFailureDetails>);
impl api_macros::openapi::OpenApiSchema for DriveHttpProblem {}
