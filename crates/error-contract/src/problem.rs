//! RFC 9457 primitives, without an HTTP server/client dependency or inner-status forwarding.
use crate::{
    codec::{self, FailureEnvelope},
    *,
};
use serde::{Deserialize, Serialize};

pub const PROBLEM_CONTENT_TYPE: &str = "application/problem+json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpFailureStatus {
    BadRequest,
    Unauthorized,
    Forbidden,
    NotFound,
    Conflict,
    Unprocessable,
    TooManyRequests,
    Internal,
    BadGateway,
    Unavailable,
    GatewayTimeout,
}
impl HttpFailureStatus {
    pub fn code(self) -> u16 {
        match self {
            Self::BadRequest => 400,
            Self::Unauthorized => 401,
            Self::Forbidden => 403,
            Self::NotFound => 404,
            Self::Conflict => 409,
            Self::Unprocessable => 422,
            Self::TooManyRequests => 429,
            Self::Internal => 500,
            Self::BadGateway => 502,
            Self::Unavailable => 503,
            Self::GatewayTimeout => 504,
        }
    }
}

/// The API boundary, not Category or an inner HTTP response, chooses this mapping.
#[derive(Debug, Clone, Copy)]
pub struct ProblemClass {
    pub status: HttpFailureStatus,
    pub type_uri: &'static str,
    pub title: &'static str,
}
impl ProblemClass {
    pub fn authorization_denied() -> Self {
        Self {
            status: HttpFailureStatus::Forbidden,
            type_uri: "urn:yoi:problem:authorization-denied",
            title: "Authorization denied",
        }
    }
    pub fn invalid_input() -> Self {
        Self {
            status: HttpFailureStatus::BadRequest,
            type_uri: "urn:yoi:problem:invalid-input",
            title: "Invalid input",
        }
    }
    pub fn conflict() -> Self {
        Self {
            status: HttpFailureStatus::Conflict,
            type_uri: "urn:yoi:problem:conflict",
            title: "Operation conflict",
        }
    }
    pub fn dependency_failure() -> Self {
        Self {
            status: HttpFailureStatus::BadGateway,
            type_uri: "urn:yoi:problem:dependency",
            title: "Dependency failure",
        }
    }
}
pub trait ApiBoundary<D> {
    fn classify(&self, details: &D) -> ProblemClass;
}

/// Untrusted wire DTO. Unknown RFC extensions are ignored; yoi_failure is mandatory here.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct ProblemDetails<D> {
    #[serde(rename = "type")]
    #[cfg_attr(feature = "json-schema", schemars(length(min = 1, max = 2048)))]
    pub type_uri: String,
    #[cfg_attr(feature = "json-schema", schemars(length(min = 1, max = 2048)))]
    pub title: String,
    #[cfg_attr(feature = "json-schema", schemars(range(min = 400, max = 599)))]
    pub status: u16,
    #[cfg_attr(feature = "json-schema", schemars(length(min = 1, max = 2048)))]
    pub detail: String,
    #[cfg_attr(feature = "json-schema", schemars(length(min = 1, max = 2048)))]
    pub instance: String,
    pub yoi_failure: FailureEnvelope<D>,
}

#[derive(Deserialize)]
struct ReaderProblem {
    #[serde(rename = "type")]
    type_uri: String,
    title: String,
    status: u16,
    detail: String,
    instance: String,
    yoi_failure: serde_json::Value,
}

pub struct ProblemResponse {
    pub status: u16,
    pub content_type: &'static str,
    body: Vec<u8>,
}
impl ProblemResponse {
    pub fn body(&self) -> &[u8] {
        &self.body
    }
}

fn validate_uri(value: &str) -> Result<(), ContractError> {
    validate_explanation(value)?;
    if !value.is_ascii()
        || value.bytes().any(|byte| {
            byte <= b' ' || byte == b'"' || byte == b'<' || byte == b'>' || byte == b'\\'
        })
    {
        return Err(ContractError::Invalid);
    }
    // Accept absolute URI and RFC URI-reference instances, not arbitrary prose.
    if !(value.contains(':') || value.starts_with('/') || value.starts_with('#')) {
        return Err(ContractError::Invalid);
    }
    Ok(())
}

pub fn to_problem_details<D: WireDetails>(
    projected: &ProjectedFailure<Browser, D>,
    api_boundary: &impl ApiBoundary<D>,
    instance: &str,
    budget: Budget,
) -> Result<ProblemResponse, ContractError> {
    let class = api_boundary.classify(projected.details());
    validate_uri(class.type_uri)?;
    validate_uri(instance)?;
    validate_explanation(class.title)?;
    // Serialize by reference; no new projection or conversion from arbitrary Display exists.
    let problem = ProblemDetails {
        type_uri: class.type_uri.into(),
        title: class.title.into(),
        status: class.status.code(),
        detail: projected.explanation().into(),
        instance: instance.into(),
        yoi_failure: FailureEnvelope {
            version: projected.envelope.version,
            required_extensions: projected.envelope.required_extensions.clone(),
            domain: projected.envelope.domain.clone(),
            reason: projected.envelope.reason.clone(),
            category: projected.envelope.category,
            details_version: projected.envelope.details_version,
            details: &projected.envelope.details,
            explanation: projected.envelope.explanation.clone(),
            context: projected.envelope.context.clone(),
        },
    };
    let body = codec::encode_bounded(&problem, budget)?;
    Ok(ProblemResponse {
        status: problem.status,
        content_type: PROBLEM_CONTENT_TYPE,
        body,
    })
}

pub fn decode_problem_details<D: WireDetails>(
    input: &[u8],
    actual_status: u16,
    content_type: &str,
    api_boundary: &impl ApiBoundary<D>,
    budget: Budget,
) -> DecodeFailure<D> {
    match decode_problem_checked(input, actual_status, content_type, api_boundary, budget) {
        Ok(result) => result,
        Err(error) => DecodeFailure::Corrupt(error),
    }
}
fn decode_problem_checked<D: WireDetails>(
    input: &[u8],
    actual_status: u16,
    content_type: &str,
    api_boundary: &impl ApiBoundary<D>,
    budget: Budget,
) -> Result<DecodeFailure<D>, ContractError> {
    codec::check_json_bounds(input, budget)?;
    if !content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .eq_ignore_ascii_case(PROBLEM_CONTENT_TYPE)
    {
        return Err(ContractError::Invalid);
    }
    let raw: ReaderProblem = serde_json::from_slice(input).map_err(|_| ContractError::Invalid)?;
    if raw.status != actual_status
        || !(400..=599).contains(&raw.status)
        || Some(raw.detail.as_str())
            != raw
                .yoi_failure
                .get("explanation")
                .and_then(serde_json::Value::as_str)
    {
        return Err(ContractError::Invalid);
    }
    validate_uri(&raw.type_uri)?;
    validate_uri(&raw.instance)?;
    validate_explanation(&raw.title)?;
    let failure_bytes = codec::encode_bounded(&raw.yoi_failure, budget)?;
    match codec::decode_checked::<D>(&failure_bytes, budget)? {
        DecodeFailure::Known(_) => {
            // Use original JSON for typed parsing, preserving duplicate-field rejection.
            let typed: ProblemDetails<D> =
                serde_json::from_slice(input).map_err(|_| ContractError::InvalidDetails)?;
            let class = api_boundary.classify(&typed.yoi_failure.details);
            if typed.status != class.status.code()
                || typed.type_uri != class.type_uri
                || typed.title != class.title
            {
                return Err(ContractError::Invalid);
            }
            codec::accept_typed_envelope(typed.yoi_failure).map(DecodeFailure::Known)
        }
        other => Ok(other),
    }
}

pub const STATIC_PROBLEM_FALLBACK: &[u8] = br#"{"type":"about:blank","title":"Internal Server Error","status":500,"detail":"Failure details unavailable."}"#;
/// Static fallback is deliberately outside the domain contract and never formats a failure.
pub fn problem_fallback(budget: Budget) -> Result<ProblemResponse, ContractError> {
    codec::check_json_bounds(STATIC_PROBLEM_FALLBACK, budget)?;
    Ok(ProblemResponse {
        status: 500,
        content_type: PROBLEM_CONTENT_TYPE,
        body: STATIC_PROBLEM_FALLBACK.to_vec(),
    })
}
