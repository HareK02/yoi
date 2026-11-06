//! Worker creation's HTTP projection is separate from its HTTP-independent result.
//! A decoded DTO is never an encoder capability: only a current visibility decision
//! can populate the cached public response. Legacy preflight routes keep their body.
use api_macros::{HttpError, HttpErrorResponse};
use runtime_api::startup::{
    DecodeStartupProblem, SessionFailureVisibility, StartupBoundary, StartupProblemDetails,
    WorkerCreateFailure, decode_startup_problem,
};
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};

use crate::RepositoryApiError;

#[derive(Debug)]
pub struct WorkerCreateApiError {
    legacy: RepositoryApiError,
    failure: Option<WorkerCreateFailure>,
    projected: Option<runtime_api::startup::ProblemResponse>,
}

impl WorkerCreateApiError {
    /// The service has authenticated and authorized the current creation caller.
    /// Visibility is a local decision; neither wire labels nor saved results supply it.
    pub fn startup(failure: WorkerCreateFailure, visibility: &SessionFailureVisibility) -> Self {
        let instance = failure
            .operation()
            .correlation
            .as_ref()
            .map(|correlation| format!("urn:yoi:worker-startup:{}", correlation.as_str()))
            .unwrap_or_default();
        let projected = failure
            .problem(StartupBoundary::Browser, visibility, &instance)
            .ok();
        let status = projected.as_ref().map_or(500, |response| response.status);
        Self {
            legacy: RepositoryApiError::new(
                status,
                "worker_startup",
                "Worker creation did not complete; inspect the typed result.",
                Vec::new(),
            ),
            failure: Some(failure),
            projected,
        }
    }

    pub fn failure(&self) -> Option<&WorkerCreateFailure> {
        self.failure.as_ref()
    }
    pub fn legacy(&self) -> &RepositoryApiError {
        &self.legacy
    }
}

impl From<RepositoryApiError> for WorkerCreateApiError {
    fn from(legacy: RepositoryApiError) -> Self {
        Self {
            legacy,
            failure: None,
            projected: None,
        }
    }
}

impl HttpError for WorkerCreateApiError {
    fn status_code(&self) -> u16 {
        self.projected
            .as_ref()
            .map_or_else(|| self.legacy.status_code(), |response| response.status)
    }

    fn custom_response(&self) -> Option<HttpErrorResponse> {
        if let Some(projected) = &self.projected {
            return Some(HttpErrorResponse {
                status: projected.status,
                content_type: projected.content_type,
                body: projected.body().to_vec().into(),
            });
        }
        // Projection failure does not serialize a possibly private internal result.
        self.failure.as_ref().map(|_| {
            error_contract::problem::problem_fallback(error_contract::Budget::new(
                error_contract::MAX_FAILURE_BYTES,
                error_contract::MAX_FAILURE_BYTES,
            ))
            .expect("static fallback fits the fixed boundary budget")
            .into()
        })
    }
}

impl api_macros::HttpRequestError for WorkerCreateApiError {
    fn from_request_rejection(status: u16, message: String) -> Self {
        <RepositoryApiError as api_macros::HttpRequestError>::from_request_rejection(
            status, message,
        )
        .into()
    }
}

impl Serialize for WorkerCreateApiError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Generated HTTP adapters use custom_response. The ordinary serializer remains
        // legacy-only and cannot serialize the retained internal startup attachment.
        if let Some(projected) = &self.projected {
            let dto: StartupProblemDetails =
                serde_json::from_slice(projected.body()).map_err(serde::ser::Error::custom)?;
            dto.serialize(serializer)
        } else {
            self.legacy.serialize(serializer)
        }
    }
}
impl<'de> Deserialize<'de> for WorkerCreateApiError {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        RepositoryApiError::deserialize(deserializer).map(Into::into)
    }
}

#[allow(dead_code)]
#[derive(JsonSchema)]
#[serde(untagged)]
enum WorkerCreateErrorWire {
    Problem(StartupProblemDetails),
    Legacy(RepositoryApiError),
}
impl JsonSchema for WorkerCreateApiError {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "WorkerCreateApiError".into()
    }
    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        WorkerCreateErrorWire::json_schema(generator)
    }
}

impl api_macros::openapi::OpenApiSchema for WorkerCreateApiError {
    fn response_schema(
        builder: &mut api_macros::openapi::OpenApiBuilder,
        content_type: &str,
    ) -> Result<serde_json::Value, api_macros::openapi::OpenApiError> {
        match content_type {
            "application/json" => builder.schema_ref::<RepositoryApiError>(),
            runtime_api::startup::PROBLEM_CONTENT_TYPE => {
                builder.schema_ref::<StartupProblemDetails>()
            }
            _ => Err(api_macros::openapi::OpenApiError::InvalidContract(
                "unsupported Worker creation error representation".into(),
            )),
        }
    }
}

impl api_macros::reqwest::HttpClientError for WorkerCreateApiError {
    fn decode_error(
        response: &api_macros::reqwest::ReceivedResponse,
    ) -> Result<Self, api_macros::reqwest::ClientFailure> {
        use api_macros::reqwest::ClientFailure;
        let content_type = response.content_type()?;
        if content_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .eq_ignore_ascii_case(runtime_api::startup::PROBLEM_CONTENT_TYPE)
        {
            return match decode_startup_problem(
                response.body(),
                response.status().as_u16(),
                content_type,
                StartupBoundary::Browser,
            ) {
                DecodeStartupProblem::Known(failure) => Ok(Self {
                    legacy: RepositoryApiError::new(
                        response.status().as_u16(),
                        "worker_startup",
                        "Worker creation returned a typed failure.",
                        Vec::new(),
                    ),
                    failure: Some(failure),
                    projected: None,
                }),
                DecodeStartupProblem::TooLarge => Err(ClientFailure::ResponseTooLarge {
                    limit: runtime_api::startup::MAX_STARTUP_PROBLEM_BYTES,
                }),
                DecodeStartupProblem::UnsupportedVersion
                | DecodeStartupProblem::UnsupportedReason => {
                    Err(ClientFailure::UnsupportedErrorResponse {
                        status: response.status().as_u16(),
                    })
                }
                DecodeStartupProblem::Invalid => Err(ClientFailure::ErrorResponseDecode {
                    status: response.status().as_u16(),
                }),
            };
        }
        let legacy = response
            .decode_json::<RepositoryApiError>(api_macros::reqwest::DecodeKind::PublicError)?;
        Ok(legacy.into())
    }
}
