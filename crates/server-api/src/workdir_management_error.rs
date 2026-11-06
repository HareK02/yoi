//! Workdir management uses the shared HTTP adapter. Runtime rejection causes are
//! independent of this Backend boundary and never attest to safe retries.
use api_macros::{HttpError, HttpErrorResponse};
use error_contract::problem::{ApiBoundary, HttpFailureStatus, ProblemClass, ProblemDetails};
use error_contract::{Browser, Budget, DecodeFailure, ProjectedFailure};
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};

use crate::RepositoryApiError;
use workdir::management_failure::WorkdirManagementFailure;

/// A receive carrier for existing domain causes, not a new failure vocabulary.
/// Untagged details preserve each owner's original identity and JSON shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum WorkdirManagementCause {
    Management(WorkdirManagementFailure),
    Session(workdir::failure::WorkdirFailureDetails),
    Runtime(protocol::control_failure::RuntimeControlCause),
}
impl error_contract::FailureCause for WorkdirManagementCause {
    fn identity(&self) -> error_contract::FailureIdentity {
        match self {
            Self::Management(cause) => cause.identity(),
            Self::Session(cause) => cause.identity(),
            Self::Runtime(cause) => cause.identity(),
        }
    }
    fn validate(&self) -> Result<(), error_contract::ContractError> {
        match self {
            Self::Management(cause) => cause.validate(),
            Self::Session(cause) => cause.validate(),
            Self::Runtime(cause) => cause.validate(),
        }
    }
}
impl error_contract::WireDetails for WorkdirManagementCause {
    fn supports(domain: &str, reason: &str, version: u16) -> bool {
        WorkdirManagementFailure::supports(domain, reason, version)
            || workdir::failure::WorkdirFailureDetails::supports(domain, reason, version)
            || protocol::control_failure::RuntimeControlCause::supports(domain, reason, version)
    }
}
macro_rules! project_cause {
    ($audience:ty) => {
        impl error_contract::ProjectFor<$audience> for WorkdirManagementCause {
            type Authorization = workdir::management_failure::WorkdirManagementVisibility;
            type Details = Self;
            fn disclose(
                &self,
                _: &error_contract::FailureFacts<Self>,
                visibility: &Self::Authorization,
            ) -> Result<error_contract::Disclosure<Self>, error_contract::ContractError> {
                if !visibility.can_view_cause {
                    return Err(error_contract::ContractError::ProjectionUnavailable);
                }
                let explanation = match self {
                    Self::Management(cause) => error_contract::project_failure::<$audience, _>(
                        &error_contract::FailureFacts::new(*cause)?,
                        visibility,
                    )?
                    .explanation()
                    .to_owned(),
                    Self::Session(cause) => error_contract::project_failure::<$audience, _>(
                        &error_contract::FailureFacts::new(*cause)?,
                        &workdir::failure::WorkdirFailureVisibility {
                            can_view_cause: true,
                        },
                    )?
                    .explanation()
                    .to_owned(),
                    Self::Runtime(cause) => error_contract::project_failure::<$audience, _>(
                        &error_contract::FailureFacts::new(*cause)?,
                        &(),
                    )?
                    .explanation()
                    .to_owned(),
                };
                Ok(error_contract::Disclosure::new(*self, explanation))
            }
        }
    };
}
project_cause!(Browser);
project_cause!(error_contract::Tool);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkdirAttachmentFailureStage {
    GrantCleanup,
    SessionClose,
    DetachBackendLink,
    RestoreBackendLink,
    SyncRuntimeAttachments,
    RefreshSession,
    RefreshWorkerProjection,
}

/// An additional cleanup/compensation observation does not replace the primary outcome.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct WorkdirAttachmentRelatedFailure {
    pub stage: WorkdirAttachmentFailureStage,
    pub http_status: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<protocol::startup_failure::StartupFailureAttachment>,
}

pub struct WorkdirManagementApiError {
    legacy: RepositoryApiError,
    cause: Option<WorkdirManagementCause>,
    related_failures: Vec<WorkdirAttachmentRelatedFailure>,
    related_failures_unavailable: bool,
    projected: Option<Vec<u8>>,
}

/// Classification belongs to these management routes, not to category or error text.
pub struct WorkdirManagementBoundary;
impl ApiBoundary<WorkdirManagementFailure> for WorkdirManagementBoundary {
    fn classify(&self, details: &WorkdirManagementFailure) -> ProblemClass {
        // These routes historically reject a failed Runtime materialization as
        // invalid input (400), including an absent reserved Workdir. Runtime's
        // own 404/409/etc. is not this operation's HTTP result.
        use workdir::management_failure::{
            WorkdirManagementOperation as Op, WorkdirManagementReason as Reason,
        };
        let status = match (details.operation, details.reason) {
            (
                Op::Attach | Op::Detach,
                Reason::AttachmentConflict | Reason::Occupied | Reason::AttachmentNotAvailable,
            ) => HttpFailureStatus::Conflict,
            (Op::Attach, Reason::AttachmentRegistryNotFound | Reason::ExternalGrantUnavailable) => {
                HttpFailureStatus::BadGateway
            }
            (Op::Attach, Reason::AttachmentGrantBoundRequired)
            | (_, Reason::GrantCreatorRequired) => HttpFailureStatus::Forbidden,
            (_, Reason::GrantStateOutcomeUnknown | Reason::AttachmentStateOutcomeUnknown) => {
                HttpFailureStatus::Unavailable
            }
            (Op::Attach | Op::Detach, Reason::AttachmentPersistenceFailed) => {
                HttpFailureStatus::Internal
            }
            (
                Op::Attach | Op::Detach,
                Reason::AttachmentProjectionFailed | Reason::AttachmentRuntimeResultUnavailable,
            ) => HttpFailureStatus::BadGateway,
            _ => HttpFailureStatus::BadRequest,
        };
        ProblemClass {
            status,
            type_uri: "urn:yoi:problem:workdir-management",
            title: "Workdir management failed",
        }
    }
}

pub struct WorkdirAttachmentBoundary;
impl ApiBoundary<WorkdirManagementCause> for WorkdirAttachmentBoundary {
    fn classify(&self, cause: &WorkdirManagementCause) -> ProblemClass {
        match cause {
            WorkdirManagementCause::Management(cause) => WorkdirManagementBoundary.classify(cause),
            // Attachment handlers historically report failed Runtime/session
            // prerequisites as 502. Inner classification never settles mutation state.
            WorkdirManagementCause::Session(_) | WorkdirManagementCause::Runtime(_) => {
                ProblemClass {
                    status: HttpFailureStatus::BadGateway,
                    type_uri: "urn:yoi:problem:workdir-attachment",
                    title: "Workdir attachment prerequisite failed",
                }
            }
        }
    }
}

fn budget() -> Budget {
    Budget::new(
        error_contract::MAX_FAILURE_BYTES,
        error_contract::MAX_FAILURE_BYTES,
    )
}

impl WorkdirManagementApiError {
    /// Only a currently authorized Browser projection can create an HTTP response.
    pub fn projected(projected: ProjectedFailure<Browser, WorkdirManagementFailure>) -> Self {
        let details = *projected.details();
        let response = api_macros::http::problem_response(
            &projected,
            &WorkdirManagementBoundary,
            "",
            budget(),
        )
        .expect("fixed boundary budget accommodates the static fallback");
        Self {
            legacy: RepositoryApiError::new(
                response.status,
                "workdir_management",
                projected.explanation(),
                Vec::new(),
            ),
            cause: Some(WorkdirManagementCause::Management(details)),
            related_failures: Vec::new(),
            related_failures_unavailable: false,
            projected: Some(response.body().to_vec()),
        }
    }
    /// Preserve an authorized primary projection beside independent compensation results.
    pub fn projected_attachment(
        mut legacy: RepositoryApiError,
        projected: ProjectedFailure<Browser, WorkdirManagementCause>,
        related_failures: Vec<WorkdirAttachmentRelatedFailure>,
    ) -> Self {
        let cause = *projected.details();
        let response = api_macros::http::problem_response(
            &projected,
            &WorkdirAttachmentBoundary,
            "",
            budget(),
        )
        .expect("closed attachment cause fits the boundary budget");
        legacy.status = response.status;
        legacy.message = projected.explanation().to_owned();
        // Only projected causes are explanations at this boundary. Legacy
        // diagnostics may contain mixed-version provider prose.
        legacy.diagnostics.clear();
        let mut problem = WorkdirManagementProblem {
            problem: serde_json::from_slice(response.body())
                .expect("shared encoder emitted a problem"),
            diagnostics: Vec::new(),
            related_failures,
            related_failures_unavailable: false,
        };
        let mut body =
            serde_json::to_vec(&problem).expect("public attachment result is serializable");
        if body.len() > error_contract::MAX_FAILURE_BYTES {
            // Optional evidence can exceed the budget, but must never erase the
            // original cause or masquerade as confirmed successful compensation.
            problem.related_failures.clear();
            problem.related_failures_unavailable = true;
            body = serde_json::to_vec(&problem).expect("bounded primary result is serializable");
        }
        Self {
            legacy,
            cause: Some(cause),
            related_failures: problem.related_failures,
            related_failures_unavailable: problem.related_failures_unavailable,
            projected: Some(body),
        }
    }

    /// Decode bounded transport data with the same adapter as generated clients.
    pub fn decode_received(
        status: u16,
        content_type: &str,
        body: &[u8],
    ) -> Result<Self, api_macros::reqwest::ClientFailure> {
        use api_macros::reqwest::ClientFailure;
        if body.len() > error_contract::MAX_FAILURE_BYTES {
            return Err(ClientFailure::ErrorResponseDecode { status });
        }
        let media = content_type.split(';').next().unwrap_or("").trim();
        if media.eq_ignore_ascii_case(error_contract::problem::PROBLEM_CONTENT_TYPE) {
            return match api_macros::http::decode_problem_response::<WorkdirManagementCause>(
                body,
                status,
                content_type,
                &WorkdirAttachmentBoundary,
                budget(),
            ) {
                DecodeFailure::Known(received) => {
                    let wire = received_attachment_metadata(body);
                    Ok(Self {
                        legacy: RepositoryApiError::new(
                            status,
                            "workdir_management",
                            "Workdir management returned a typed failure.",
                            Vec::new(),
                        ),
                        cause: Some(*received.facts().cause()),
                        related_failures: wire.0,
                        related_failures_unavailable: wire.1,
                        projected: None,
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
        let mut legacy: RepositoryApiError = serde_json::from_slice(body)
            .map_err(|_| ClientFailure::ErrorResponseDecode { status })?;
        legacy.status = status;
        Ok(legacy.into())
    }
    pub fn details(&self) -> Option<&WorkdirManagementFailure> {
        match self.cause.as_ref() {
            Some(WorkdirManagementCause::Management(cause)) => Some(cause),
            _ => None,
        }
    }
    pub fn cause(&self) -> Option<&WorkdirManagementCause> {
        self.cause.as_ref()
    }
    pub fn related_failures(&self) -> &[WorkdirAttachmentRelatedFailure] {
        &self.related_failures
    }
    pub fn related_failures_unavailable(&self) -> bool {
        self.related_failures_unavailable
    }
    pub fn legacy(&self) -> &RepositoryApiError {
        &self.legacy
    }
}
impl From<RepositoryApiError> for WorkdirManagementApiError {
    fn from(legacy: RepositoryApiError) -> Self {
        Self {
            legacy,
            cause: None,
            related_failures: Vec::new(),
            related_failures_unavailable: false,
            projected: None,
        }
    }
}
impl std::fmt::Debug for WorkdirManagementApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkdirManagementApiError")
            .field("status", &self.status_code())
            .field("cause", &self.cause)
            .finish_non_exhaustive()
    }
}
impl HttpError for WorkdirManagementApiError {
    fn status_code(&self) -> u16 {
        self.legacy.status_code()
    }
    fn custom_response(&self) -> Option<HttpErrorResponse> {
        self.projected.as_ref().map(|p| HttpErrorResponse {
            status: self.legacy.status_code(),
            content_type: error_contract::problem::PROBLEM_CONTENT_TYPE,
            body: p.clone().into(),
        })
    }
}
impl api_macros::HttpRequestError for WorkdirManagementApiError {
    fn from_request_rejection(status: u16, message: String) -> Self {
        <RepositoryApiError as api_macros::HttpRequestError>::from_request_rejection(
            status, message,
        )
        .into()
    }
}
impl Serialize for WorkdirManagementApiError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Received details alone cannot create an encoder capability.
        match &self.projected {
            Some(p) => serde_json::from_slice::<WorkdirManagementProblem>(p)
                .map_err(serde::ser::Error::custom)?
                .serialize(serializer),
            None => self.legacy.serialize(serializer),
        }
    }
}
impl<'de> Deserialize<'de> for WorkdirManagementApiError {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        RepositoryApiError::deserialize(deserializer).map(Into::into)
    }
}
#[allow(dead_code)]
#[derive(JsonSchema)]
#[serde(untagged)]
enum WorkdirManagementErrorWire {
    Problem(WorkdirManagementProblem),
    Legacy(RepositoryApiError),
}
impl JsonSchema for WorkdirManagementApiError {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "WorkdirManagementApiError".into()
    }
    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        WorkdirManagementErrorWire::json_schema(generator)
    }
}
impl api_macros::openapi::OpenApiSchema for WorkdirManagementApiError {
    fn response_schema(
        builder: &mut api_macros::openapi::OpenApiBuilder,
        content_type: &str,
    ) -> Result<serde_json::Value, api_macros::openapi::OpenApiError> {
        match content_type {
            "application/json" => builder.schema_ref::<RepositoryApiError>(),
            error_contract::problem::PROBLEM_CONTENT_TYPE => {
                builder.schema_ref::<WorkdirManagementProblem>()
            }
            _ => Err(api_macros::openapi::OpenApiError::InvalidContract(
                "unsupported Workdir management error representation".into(),
            )),
        }
    }
}
impl api_macros::reqwest::HttpClientError for WorkdirManagementApiError {
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

#[derive(Serialize, Deserialize, JsonSchema)]
struct WorkdirManagementProblem {
    #[serde(flatten)]
    problem: ProblemDetails<WorkdirManagementCause>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    diagnostics: Vec<crate::Diagnostic>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    related_failures: Vec<WorkdirAttachmentRelatedFailure>,
    #[serde(default, skip_serializing_if = "is_false")]
    related_failures_unavailable: bool,
}
impl api_macros::openapi::OpenApiSchema for WorkdirManagementProblem {}

fn is_false(value: &bool) -> bool {
    !value
}

// This optional domain metadata is decoded independently of the already validated
// primary envelope. A future stage or damaged secondary row cannot erase it.
fn received_attachment_metadata(body: &[u8]) -> (Vec<WorkdirAttachmentRelatedFailure>, bool) {
    #[derive(Deserialize)]
    struct Metadata {
        #[serde(default)]
        related_failures: Vec<serde_json::Value>,
        #[serde(default)]
        related_failures_unavailable: bool,
    }
    let Ok(metadata) = serde_json::from_slice::<Metadata>(body) else {
        return (Vec::new(), true);
    };
    let mut unavailable = metadata.related_failures_unavailable;
    let failures = metadata
        .related_failures
        .into_iter()
        .filter_map(|value| match serde_json::from_value(value) {
            Ok(failure) => Some(failure),
            Err(_) => {
                unavailable = true;
                None
            }
        })
        .collect();
    (failures, unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use error_contract::{FailureFacts, Tool, project_failure};
    use workdir::management_failure::{
        WorkdirManagementOperation, WorkdirManagementReason, WorkdirManagementVisibility,
    };

    #[test]
    fn backend_http_keeps_its_class_separate_from_runtime_cause_and_tool_explanation() {
        let cause = WorkdirManagementFailure {
            operation: WorkdirManagementOperation::Create,
            reason: WorkdirManagementReason::NotFound,
        };
        let facts = FailureFacts::new(cause).unwrap();
        let visibility = WorkdirManagementVisibility {
            can_view_cause: true,
        };
        let browser = project_failure::<Browser, _>(&facts, &visibility).unwrap();
        let tool = project_failure::<Tool, _>(&facts, &visibility).unwrap();
        let error = WorkdirManagementApiError::projected(browser);
        let response = error.custom_response().unwrap();
        assert_eq!(response.status, 400);
        assert_eq!(cause.reason.runtime_http_status(), 404);
        let DecodeFailure::Known(received) =
            api_macros::http::decode_problem_response::<WorkdirManagementFailure>(
                &response.body,
                response.status,
                response.content_type,
                &WorkdirManagementBoundary,
                budget(),
            )
        else {
            panic!("management cause lost");
        };
        assert_eq!(received.facts().cause(), tool.details());
        let raw: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(raw["detail"], tool.explanation());
        assert!(matches!(
            api_macros::http::decode_problem_response::<WorkdirManagementFailure>(
                &response.body,
                404,
                response.content_type,
                &WorkdirManagementBoundary,
                budget(),
            ),
            DecodeFailure::Corrupt(_)
        ));
    }
}
