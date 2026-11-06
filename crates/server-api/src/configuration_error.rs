//! Configuration adoption HTTP errors use the shared adapter, without exposing the local source.
use api_macros::{HttpError, HttpErrorResponse};
use error_contract::problem::{ApiBoundary, HttpFailureStatus, ProblemClass, ProblemDetails};
use error_contract::{Browser, Budget, DecodeFailure, ProjectedFailure};
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};

use crate::{
    RepositoryApiError,
    configuration_failure::{ConfigurationFailureDetails, ConfigurationFailureReason},
};

pub struct ConfigurationApiError {
    legacy: RepositoryApiError,
    details: Option<ConfigurationFailureDetails>,
    projected: Option<error_contract::problem::ProblemResponse>,
}

/// Classification belongs to these management routes, not to category or error text.
pub struct ConfigurationBoundary;
impl ApiBoundary<ConfigurationFailureDetails> for ConfigurationBoundary {
    fn classify(&self, details: &ConfigurationFailureDetails) -> ProblemClass {
        use ConfigurationFailureReason::*;
        let status = match details.reason {
            OwnerRequired => HttpFailureStatus::Forbidden,
            PublicationDispatchUnavailable | PublicationProjectionUnavailable => {
                HttpFailureStatus::Internal
            }
            CatalogChanged
            | EditingHeadChanged
            | LegacyExportChanged
            | SourceContentChanged
            | WorkspaceInactive
            | WorkspaceDraining
            | RequestIdentityConflict => HttpFailureStatus::Conflict,
            InvalidPath
            | InvalidImport
            | ImportEscape
            | AlreadyExists
            | NotFound
            | EntryConflict
            | EntryDigestMismatch
            | EntryPathMismatch
            | TreeDigestMismatch
            | PathChangedMoreThanOnce
            | DuplicatePath
            | DuplicateSchemaProvider
            | DuplicateSchemaNamespace
            | InvalidSchemaContribution
            | LimitExceeded => HttpFailureStatus::BadRequest,
        };
        ProblemClass {
            status,
            type_uri: "urn:yoi:problem:configuration",
            title: "Configuration adoption failed",
        }
    }
}

fn budget() -> Budget {
    Budget::new(
        error_contract::MAX_FAILURE_BYTES,
        error_contract::MAX_FAILURE_BYTES,
    )
}

impl ConfigurationApiError {
    /// Only a currently authorized Browser projection can create an HTTP response.
    pub fn projected(projected: ProjectedFailure<Browser, ConfigurationFailureDetails>) -> Self {
        let details = *projected.details();
        let response =
            api_macros::http::problem_response(&projected, &ConfigurationBoundary, "", budget())
                .expect("fixed boundary budget accommodates the static fallback");
        Self {
            legacy: RepositoryApiError::new(
                response.status,
                "configuration",
                projected.explanation(),
                Vec::new(),
            ),
            details: Some(details),
            projected: Some(response),
        }
    }
    pub fn details(&self) -> Option<&ConfigurationFailureDetails> {
        self.details.as_ref()
    }
    pub fn legacy(&self) -> &RepositoryApiError {
        &self.legacy
    }
}
impl From<RepositoryApiError> for ConfigurationApiError {
    fn from(legacy: RepositoryApiError) -> Self {
        Self {
            legacy,
            details: None,
            projected: None,
        }
    }
}
impl std::fmt::Debug for ConfigurationApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigurationApiError")
            .field("status", &self.status_code())
            .field("details", &self.details)
            .finish_non_exhaustive()
    }
}
impl HttpError for ConfigurationApiError {
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
impl api_macros::HttpRequestError for ConfigurationApiError {
    fn from_request_rejection(status: u16, message: String) -> Self {
        <RepositoryApiError as api_macros::HttpRequestError>::from_request_rejection(
            status, message,
        )
        .into()
    }
}
impl Serialize for ConfigurationApiError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Received details alone cannot create an encoder capability.
        match &self.projected {
            Some(p) => {
                serde_json::from_slice::<ProblemDetails<ConfigurationFailureDetails>>(p.body())
                    .map_err(serde::ser::Error::custom)?
                    .serialize(serializer)
            }
            None => self.legacy.serialize(serializer),
        }
    }
}
impl<'de> Deserialize<'de> for ConfigurationApiError {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        RepositoryApiError::deserialize(deserializer).map(Into::into)
    }
}
#[allow(dead_code)]
#[derive(JsonSchema)]
#[serde(untagged)]
enum ConfigurationErrorWire {
    Problem(ProblemDetails<ConfigurationFailureDetails>),
    Legacy(RepositoryApiError),
}
impl JsonSchema for ConfigurationApiError {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ConfigurationApiError".into()
    }
    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        ConfigurationErrorWire::json_schema(generator)
    }
}
impl api_macros::openapi::OpenApiSchema for ConfigurationApiError {
    fn response_schema(
        builder: &mut api_macros::openapi::OpenApiBuilder,
        content_type: &str,
    ) -> Result<serde_json::Value, api_macros::openapi::OpenApiError> {
        match content_type {
            "application/json" => builder.schema_ref::<RepositoryApiError>(),
            error_contract::problem::PROBLEM_CONTENT_TYPE => {
                builder.schema_ref::<ConfigurationProblem>()
            }
            _ => Err(api_macros::openapi::OpenApiError::InvalidContract(
                "unsupported Configuration adoption error representation".into(),
            )),
        }
    }
}
impl api_macros::reqwest::HttpClientError for ConfigurationApiError {
    fn decode_error(
        response: &api_macros::reqwest::ReceivedResponse,
    ) -> Result<Self, api_macros::reqwest::ClientFailure> {
        use api_macros::reqwest::ClientFailure;
        let status = response.status().as_u16();
        let content_type = response.content_type()?;
        let media = content_type.split(';').next().unwrap_or("").trim();
        if media.eq_ignore_ascii_case(error_contract::problem::PROBLEM_CONTENT_TYPE) {
            return match response
                .decode_problem::<ConfigurationFailureDetails>(&ConfigurationBoundary, budget())
            {
                DecodeFailure::Known(received) => Ok(Self {
                    legacy: RepositoryApiError::new(
                        status,
                        "configuration",
                        "Configuration adoption returned a typed failure.",
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

#[derive(JsonSchema)]
#[serde(transparent)]
struct ConfigurationProblem(ProblemDetails<ConfigurationFailureDetails>);
impl api_macros::openapi::OpenApiSchema for ConfigurationProblem {}
