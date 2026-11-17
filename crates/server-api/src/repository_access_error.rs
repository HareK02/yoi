//! Repository-access HTTP errors use the shared adapter, without exposing the local source.
use api_macros::{HttpError, HttpErrorResponse};
use error_contract::problem::{ApiBoundary, HttpFailureStatus, ProblemClass, ProblemDetails};
use error_contract::{Browser, Budget, DecodeFailure, ProjectedFailure};
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};

use crate::{
    RepositoryApiError,
    repository_access_failure::{RepositoryAccessDetails, RepositoryAccessReason},
};

pub struct RepositoryAccessApiError {
    legacy: RepositoryApiError,
    details: Option<RepositoryAccessDetails>,
    projected: Option<error_contract::problem::ProblemResponse>,
}

/// Classification belongs to these management routes, not to category or error text.
pub struct RepositoryAccessBoundary;
impl ApiBoundary<RepositoryAccessDetails> for RepositoryAccessBoundary {
    fn classify(&self, details: &RepositoryAccessDetails) -> ProblemClass {
        use RepositoryAccessReason::*;
        let status = match details.reason {
            CredentialNotFound | HostTrustNotFound => HttpFailureStatus::NotFound,
            CredentialAlreadyExists
            | WorkspaceDefaultCredentialImmutable
            | CredentialObservationChanged
            | CredentialKeyChanged
            | CredentialReferenced
            | CredentialRetained
            | HostTrustObservationChanged
            | HostTrustKeyChanged
            | HostTrustReferenced => HttpFailureStatus::Conflict,
            PlainHttpSourceUnsupported
            | UnknownRepository
            | SshSourceRequired
            | BindingCredentialNotFound
            | CredentialInactive
            | BindingHostTrustNotFound
            | HostEndpointMismatch
            | AmbiguousHostTrust
            | CredentialUnavailableOrInactive
            | HostTrustUnavailable => HttpFailureStatus::BadRequest,
        };
        ProblemClass {
            status,
            type_uri: "urn:yoi:problem:repository-access",
            title: "Repository access failed",
        }
    }
}

fn budget() -> Budget {
    Budget::new(
        error_contract::MAX_FAILURE_BYTES,
        error_contract::MAX_FAILURE_BYTES,
    )
}

impl RepositoryAccessApiError {
    /// Only a currently authorized Browser projection can create an HTTP response.
    pub fn projected(projected: ProjectedFailure<Browser, RepositoryAccessDetails>) -> Self {
        let details = *projected.details();
        let response =
            api_macros::http::problem_response(&projected, &RepositoryAccessBoundary, "", budget())
                .expect("fixed boundary budget accommodates the static fallback");
        Self {
            legacy: RepositoryApiError::new(
                response.status,
                "repository_access",
                projected.explanation(),
                Vec::new(),
            ),
            details: Some(details),
            projected: Some(response),
        }
    }
    pub fn details(&self) -> Option<&RepositoryAccessDetails> {
        self.details.as_ref()
    }
    pub fn legacy(&self) -> &RepositoryApiError {
        &self.legacy
    }
}
impl From<RepositoryApiError> for RepositoryAccessApiError {
    fn from(legacy: RepositoryApiError) -> Self {
        Self {
            legacy,
            details: None,
            projected: None,
        }
    }
}
impl std::fmt::Debug for RepositoryAccessApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RepositoryAccessApiError")
            .field("status", &self.status_code())
            .field("details", &self.details)
            .finish_non_exhaustive()
    }
}
impl HttpError for RepositoryAccessApiError {
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
impl api_macros::HttpRequestError for RepositoryAccessApiError {
    fn from_request_rejection(status: u16, message: String) -> Self {
        <RepositoryApiError as api_macros::HttpRequestError>::from_request_rejection(
            status, message,
        )
        .into()
    }
}
impl Serialize for RepositoryAccessApiError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Received details alone cannot create an encoder capability.
        match &self.projected {
            Some(p) => serde_json::from_slice::<ProblemDetails<RepositoryAccessDetails>>(p.body())
                .map_err(serde::ser::Error::custom)?
                .serialize(serializer),
            None => self.legacy.serialize(serializer),
        }
    }
}
impl<'de> Deserialize<'de> for RepositoryAccessApiError {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        RepositoryApiError::deserialize(deserializer).map(Into::into)
    }
}
#[allow(dead_code)]
#[derive(JsonSchema)]
#[serde(untagged)]
enum RepositoryAccessErrorWire {
    Problem(ProblemDetails<RepositoryAccessDetails>),
    Legacy(RepositoryApiError),
}
impl JsonSchema for RepositoryAccessApiError {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "RepositoryAccessApiError".into()
    }
    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        RepositoryAccessErrorWire::json_schema(generator)
    }
}
impl api_macros::openapi::OpenApiSchema for RepositoryAccessApiError {
    fn response_schema(
        builder: &mut api_macros::openapi::OpenApiBuilder,
        content_type: &str,
    ) -> Result<serde_json::Value, api_macros::openapi::OpenApiError> {
        match content_type {
            "application/json" => builder.schema_ref::<RepositoryApiError>(),
            error_contract::problem::PROBLEM_CONTENT_TYPE => {
                builder.schema_ref::<RepositoryAccessProblem>()
            }
            _ => Err(api_macros::openapi::OpenApiError::InvalidContract(
                "unsupported Repository access error representation".into(),
            )),
        }
    }
}
impl api_macros::reqwest::HttpClientError for RepositoryAccessApiError {
    fn decode_error(
        response: &api_macros::reqwest::ReceivedResponse,
    ) -> Result<Self, api_macros::reqwest::ClientFailure> {
        use api_macros::reqwest::ClientFailure;
        let status = response.status().as_u16();
        let content_type = response.content_type()?;
        let media = content_type.split(';').next().unwrap_or("").trim();
        if media.eq_ignore_ascii_case(error_contract::problem::PROBLEM_CONTENT_TYPE) {
            return match response
                .decode_problem::<RepositoryAccessDetails>(&RepositoryAccessBoundary, budget())
            {
                DecodeFailure::Known(received) => Ok(Self {
                    legacy: RepositoryApiError::new(
                        status,
                        "repository_access",
                        "Repository access returned a typed failure.",
                        Vec::new(),
                    ),
                    details: Some(*received.facts().cause()),
                    projected: None,
                }),
                DecodeFailure::Unsupported(_) => {
                    Err(ClientFailure::UnsupportedErrorResponse { status })
                }
                DecodeFailure::Corrupt(_) => Err(ClientFailure::ErrorResponseDecode { status }),
            };
        }
        if !media.eq_ignore_ascii_case("application/json") {
            return Err(ClientFailure::UnsupportedErrorResponse { status });
        }
        let mut legacy: RepositoryApiError = serde_json::from_slice(response.body())
            .map_err(|_| ClientFailure::ErrorResponseDecode { status })?;
        legacy.status = status;
        Ok(legacy.into())
    }
}

// Schema-only wrapper: its field defines the OpenAPI payload, not a runtime value.
#[expect(
    dead_code,
    reason = "payload field is consumed by JsonSchema generation"
)]
#[derive(JsonSchema)]
#[serde(transparent)]
struct RepositoryAccessProblem(ProblemDetails<RepositoryAccessDetails>);
impl api_macros::openapi::OpenApiSchema for RepositoryAccessProblem {}
