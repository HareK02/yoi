//! Runtime API HTTP integration. No HTTP status is converted into execution facts.
use api_macros::reqwest::{ClientFailure, HttpClientError, ReceivedResponse};
use error_contract::{DecodeFailure, InternalTransfer};
use serde::Deserialize;

use super::*;
use crate::RuntimeApiError;

fn unknown_operation() -> StartupOperationFacts {
    StartupOperationFacts {
        acceptance: CreationAcceptance::Unknown,
        execution: ExecutionOutcome::Unknown,
        finalize: None,
        backend_compensation: None,
        correlation: None,
    }
}

/// RuntimeManagementApi invokes this only after checking authenticated Runtime
/// scope. That scope permits the minimum public-safe cause schema, not arbitrary
/// internal-transfer details. Resolve the public projection afresh on each call;
/// no stored/received projection is relabeled as Browser authorization.
pub(crate) fn runtime_error_response(
    error: &RuntimeApiError,
) -> Option<api_macros::HttpErrorResponse> {
    let legacy;
    let failure = match error.create_failure() {
        Some(failure) => failure,
        None => {
            let startup = error.error.startup.as_ref()?;
            legacy = WorkerCreateFailure::new(startup.clone(), unknown_operation());
            match &legacy {
                Ok(failure) => failure,
                Err(_) => return Some(static_fallback()),
            }
        }
    };
    let visibility = SessionFailureVisibility {
        can_view_cause: true,
    };
    Some(
        match failure.problem(StartupBoundary::Runtime, &visibility, "") {
            Ok(response) => api_macros::HttpErrorResponse {
                status: response.status,
                content_type: response.content_type,
                body: response.into_body().into(),
            },
            // Never expose the legacy/raw attachment if projection cannot be made.
            Err(_) => static_fallback(),
        },
    )
}

fn static_fallback() -> api_macros::HttpErrorResponse {
    api_macros::HttpErrorResponse {
        status: 500,
        content_type: PROBLEM_CONTENT_TYPE,
        body: error_contract::problem::STATIC_PROBLEM_FALLBACK
            .to_vec()
            .into(),
    }
}

fn normalized_error(
    status: u16,
    failure: WorkerCreateFailure,
) -> Result<RuntimeApiError, ClientFailure> {
    // Received text and legacy labels are not sources of local explanations.
    // InternalTransfer is an internal observation policy, not Browser authority.
    let projected = failure
        .startup()
        .primary
        .project_for::<InternalTransfer, StartupCause>(&())
        .map_err(|_| ClientFailure::ErrorResponseDecode { status })?;
    let identity = projected.identity();
    let mut error = RuntimeApiError::new(
        status,
        format!("{}.{}", identity.domain, identity.reason),
        projected.explanation(),
    );
    error.error.startup = Some(failure.startup().clone());
    Ok(error.with_create_failure(failure))
}

fn local_observation(
    status: u16,
    reason: StartupBoundaryReason,
) -> Result<RuntimeApiError, ClientFailure> {
    let failure = WorkerCreateFailure::from_cause(
        StartupCause::Boundary(StartupBoundaryCause { reason }),
        unknown_operation(),
    )
    .map_err(|_| ClientFailure::ErrorResponseDecode { status })?;
    normalized_error(status, failure)
}

// A deliberately narrow legacy schema. Bounded typed parsing avoids arbitrary JSON
// allocation; startup attachment strings remain opaque until domain codec validation.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyError {
    error: LegacyDetail,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyDetail {
    code: String,
    message: String,
    startup: Option<WorkerStartupFailure>,
}

impl HttpClientError for RuntimeApiError {
    fn decode_error(response: &ReceivedResponse) -> Result<Self, ClientFailure> {
        let status = response.status().as_u16();
        if !(400..=599).contains(&status) {
            return Err(ClientFailure::ErrorResponseDecode { status });
        }
        // The generated client has already bounded collection; enforce this
        // smaller contract limit independently before any schema allocation.
        if response.body().len() > MAX_STARTUP_PROBLEM_BYTES {
            return local_observation(status, StartupBoundaryReason::TooLarge);
        }
        let content_type = match response.content_type() {
            Ok(content_type) => content_type,
            Err(_) => return local_observation(status, StartupBoundaryReason::InvalidResponse),
        };
        let media_type = content_type.split(';').next().unwrap_or("").trim();
        if media_type.eq_ignore_ascii_case(PROBLEM_CONTENT_TYPE) {
            return match decode_startup_problem(
                response.body(),
                status,
                content_type,
                StartupBoundary::Runtime,
            ) {
                DecodeStartupProblem::Known(failure) => normalized_error(status, failure),
                DecodeStartupProblem::UnsupportedVersion => {
                    local_observation(status, StartupBoundaryReason::UnsupportedVersion)
                }
                DecodeStartupProblem::UnsupportedReason => {
                    local_observation(status, StartupBoundaryReason::UnsupportedReason)
                }
                DecodeStartupProblem::Invalid => {
                    local_observation(status, StartupBoundaryReason::InvalidResponse)
                }
                DecodeStartupProblem::TooLarge => {
                    local_observation(status, StartupBoundaryReason::TooLarge)
                }
            };
        }
        if !media_type.eq_ignore_ascii_case("application/json") {
            return local_observation(status, StartupBoundaryReason::InvalidResponse);
        }
        // The typed legacy parser has fixed nesting and rejects unknown fields.
        // Its total allocation/opaque attachment size is capped by the byte limit.
        let legacy = match serde_json::from_slice::<LegacyError>(response.body()) {
            Ok(legacy) => legacy.error,
            Err(_) => return local_observation(status, StartupBoundaryReason::Decode),
        };
        if error_contract::validate_identifier(&legacy.code).is_err()
            || error_contract::validate_explanation(&legacy.message).is_err()
        {
            return local_observation(status, StartupBoundaryReason::InvalidResponse);
        }
        let Some(mut startup) = legacy.startup else {
            // Preserve ordinary legacy errors exactly, including their actual status.
            return Ok(RuntimeApiError::new(status, legacy.code, legacy.message));
        };
        let projected = match startup.primary.decode::<StartupCause>() {
            DecodeFailure::Known(received) => match error_contract::project_failure::<
                InternalTransfer,
                _,
            >(received.facts(), &())
            {
                Ok(projected) => projected,
                Err(_) => return local_observation(status, StartupBoundaryReason::InvalidResponse),
            },
            DecodeFailure::Corrupt(error_contract::ContractError::TooLarge) => {
                return local_observation(status, StartupBoundaryReason::TooLarge);
            }
            // The legacy attachment codec intentionally exposes no unsupported
            // metadata. Do not guess a reason/version from that opaque evidence.
            _ => return local_observation(status, StartupBoundaryReason::InvalidResponse),
        };
        startup.primary = match StartupFailureAttachment::from_projected(&projected) {
            Ok(primary) => primary,
            Err(_) => return local_observation(status, StartupBoundaryReason::InvalidResponse),
        };
        match WorkerCreateFailure::new(startup, unknown_operation()) {
            Ok(failure) => normalized_error(status, failure),
            Err(_) => local_observation(status, StartupBoundaryReason::InvalidResponse),
        }
    }
}
