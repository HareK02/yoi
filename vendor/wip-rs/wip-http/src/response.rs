use std::error::Error;
use std::fmt::{self, Display, Formatter};

use http::StatusCode;
use serde_json::{Map, Value as Json};
use wip_protocol::{ProtocolError, ProtocolErrorCode, ProtocolInteraction};

use crate::{BodyKind, CodecError, Limits, json};

/// A successfully classified client response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodedResponse<T> {
    /// A successful direct response body.
    Success(T),
    /// A canonical Host protocol failure.
    ProtocolFailure {
        /// The authoritative protocol error decoded from the envelope.
        error: ProtocolError,
        /// A diagnostic when the HTTP status differs from the canonical mapping.
        status_mismatch: Option<StatusMismatch>,
    },
}

/// Diagnostic evidence that HTTP metadata disagrees with a valid error envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusMismatch {
    /// Status received from the peer.
    pub actual: StatusCode,
    /// Canonical status for the envelope's protocol code.
    pub expected: StatusCode,
}

/// Class of non-protocol HTTP failure returned without a valid WIP envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportFailureKind {
    /// HTTP 401, commonly emitted by authentication infrastructure.
    AuthenticationRequired,
    /// HTTP 403, commonly emitted by endpoint policy or infrastructure.
    EndpointForbidden,
    /// The response uses a content coding the client does not support.
    UnsupportedContentCoding,
    /// Another non-success HTTP response without a WIP error envelope.
    HttpStatus,
}

/// A non-success HTTP response that is not a valid WIP protocol failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportBindingFailure {
    /// HTTP status returned by the peer or intermediary.
    pub status: StatusCode,
    /// Coarse classification which does not invent a protocol error code.
    pub kind: TransportFailureKind,
}

/// Why a response violates the WIP v1 response contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidResponseKind {
    /// A 2xx response contained an error envelope.
    ErrorEnvelopeOnSuccessStatus,
    /// A non-2xx response contained a valid success body.
    SuccessBodyOnErrorStatus,
    /// A valid error envelope used a code forbidden for the interaction.
    DisallowedProtocolCode,
    /// A valid success body used a 2xx status other than canonical HTTP 200.
    NonCanonicalSuccessStatus,
    /// A 2xx body was malformed or semantically invalid.
    InvalidSuccessBody,
}

/// A local failure while classifying or validating a peer response.
#[derive(Debug)]
pub enum ClientResponseError {
    /// The HTTP exchange failed outside the protocol envelope.
    TransportBinding(TransportBindingFailure),
    /// The peer supplied a contradictory or invalid WIP response.
    InvalidResponse {
        /// Stable response-failure category.
        kind: InvalidResponseKind,
        /// Bounded diagnostic from local decoding.
        detail: String,
    },
    /// The descriptor is structurally valid but uses an unsupported format.
    UnsupportedDescriptorFormat {
        /// Format identifier received from the Host.
        format: String,
    },
}

impl ClientResponseError {
    pub(crate) fn invalid(kind: InvalidResponseKind, detail: impl Into<String>) -> Self {
        Self::InvalidResponse {
            kind,
            detail: detail.into(),
        }
    }
}

impl Display for ClientResponseError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::TransportBinding(failure) => write!(
                formatter,
                "transport binding failure ({:?}, HTTP {})",
                failure.kind, failure.status
            ),
            Self::InvalidResponse { kind, detail } => {
                write!(formatter, "invalid WIP response ({kind:?}): {detail}")
            }
            Self::UnsupportedDescriptorFormat { format } => {
                write!(
                    formatter,
                    "unsupported interface descriptor format `{format}`"
                )
            }
        }
    }
}

impl Error for ClientResponseError {}

/// Returns the canonical HTTP status for a wire-level protocol error.
#[must_use]
pub const fn status_for_error(code: ProtocolErrorCode) -> StatusCode {
    match code {
        ProtocolErrorCode::InvalidRequest => StatusCode::BAD_REQUEST,
        ProtocolErrorCode::NotFound | ProtocolErrorCode::InterfaceNotFound => StatusCode::NOT_FOUND,
        ProtocolErrorCode::InterfaceMismatch => StatusCode::CONFLICT,
        ProtocolErrorCode::OperationNotFound => StatusCode::NOT_FOUND,
        ProtocolErrorCode::InvalidArguments => StatusCode::UNPROCESSABLE_ENTITY,
        ProtocolErrorCode::ValidatorRequired | ProtocolErrorCode::InterfaceValidatorRequired => {
            StatusCode::PRECONDITION_REQUIRED
        }
        ProtocolErrorCode::ValidatorMismatch | ProtocolErrorCode::InterfaceValidatorMismatch => {
            StatusCode::PRECONDITION_FAILED
        }
        ProtocolErrorCode::PermissionDenied => StatusCode::FORBIDDEN,
        ProtocolErrorCode::ResourceLimitExceeded => StatusCode::UNPROCESSABLE_ENTITY,
        ProtocolErrorCode::Internal | ProtocolErrorCode::OperationOutcomeUnknown => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

fn status_matches_error(status: StatusCode, code: ProtocolErrorCode) -> bool {
    status == status_for_error(code)
}

/// Returns the stable snake-case wire spelling of a protocol code.
#[must_use]
pub const fn error_code_name(code: ProtocolErrorCode) -> &'static str {
    match code {
        ProtocolErrorCode::InvalidRequest => "invalid_request",
        ProtocolErrorCode::NotFound => "not_found",
        ProtocolErrorCode::InterfaceNotFound => "interface_not_found",
        ProtocolErrorCode::InterfaceMismatch => "interface_mismatch",
        ProtocolErrorCode::OperationNotFound => "operation_not_found",
        ProtocolErrorCode::InvalidArguments => "invalid_arguments",
        ProtocolErrorCode::ValidatorRequired => "validator_required",
        ProtocolErrorCode::ValidatorMismatch => "validator_mismatch",
        ProtocolErrorCode::InterfaceValidatorRequired => "interface_validator_required",
        ProtocolErrorCode::InterfaceValidatorMismatch => "interface_validator_mismatch",
        ProtocolErrorCode::PermissionDenied => "permission_denied",
        ProtocolErrorCode::ResourceLimitExceeded => "resource_limit_exceeded",
        ProtocolErrorCode::Internal => "internal",
        ProtocolErrorCode::OperationOutcomeUnknown => "operation_outcome_unknown",
    }
}

fn error_code_from_name(name: &str) -> Option<ProtocolErrorCode> {
    Some(match name {
        "invalid_request" => ProtocolErrorCode::InvalidRequest,
        "not_found" => ProtocolErrorCode::NotFound,
        "interface_not_found" => ProtocolErrorCode::InterfaceNotFound,
        "interface_mismatch" => ProtocolErrorCode::InterfaceMismatch,
        "operation_not_found" => ProtocolErrorCode::OperationNotFound,
        "invalid_arguments" => ProtocolErrorCode::InvalidArguments,
        "validator_required" => ProtocolErrorCode::ValidatorRequired,
        "validator_mismatch" => ProtocolErrorCode::ValidatorMismatch,
        "interface_validator_required" => ProtocolErrorCode::InterfaceValidatorRequired,
        "interface_validator_mismatch" => ProtocolErrorCode::InterfaceValidatorMismatch,
        "permission_denied" => ProtocolErrorCode::PermissionDenied,
        "resource_limit_exceeded" => ProtocolErrorCode::ResourceLimitExceeded,
        "internal" => ProtocolErrorCode::Internal,
        "operation_outcome_unknown" => ProtocolErrorCode::OperationOutcomeUnknown,
        _ => return None,
    })
}

pub(crate) fn error_to_json(error: &ProtocolError) -> Json {
    serde_json::json!({
        "error": {
            "code": error_code_name(error.code),
            "message": error.message,
        }
    })
}

fn protocol_error_from_json(value: &Json) -> Result<ProtocolError, CodecError> {
    let fields = value
        .as_object()
        .ok_or_else(|| invalid_field("error", "expected object"))?;
    exact(fields, &["code", "message"], "error")?;
    let code_name = required_string(fields, "code")?;
    let code = error_code_from_name(code_name)
        .ok_or_else(|| invalid_field("error.code", "unknown protocol error code"))?;
    let message = required_string(fields, "message")?.to_owned();
    Ok(ProtocolError { code, message })
}

fn envelope_error_from_json(value: &Json) -> Result<Option<ProtocolError>, CodecError> {
    let Some(fields) = value.as_object() else {
        return Ok(None);
    };
    if fields.len() != 1 || !fields.contains_key("error") {
        return Ok(None);
    }
    protocol_error_from_json(&fields["error"]).map(Some)
}

fn exact(fields: &Map<String, Json>, allowed: &[&str], parent: &str) -> Result<(), CodecError> {
    for name in fields.keys() {
        if !allowed.contains(&name.as_str()) {
            return Err(CodecError::UnknownField {
                field: format!("{parent}.{name}"),
            });
        }
    }
    for name in allowed {
        if !fields.contains_key(*name) {
            return Err(CodecError::MissingField {
                field: format!("{parent}.{name}"),
            });
        }
    }
    Ok(())
}

fn required_string<'a>(fields: &'a Map<String, Json>, name: &str) -> Result<&'a str, CodecError> {
    fields
        .get(name)
        .and_then(Json::as_str)
        .ok_or_else(|| invalid_field(&format!("error.{name}"), "expected string"))
}

fn invalid_field(field: &str, reason: &str) -> CodecError {
    CodecError::InvalidField {
        field: field.into(),
        reason: reason.into(),
    }
}

pub(crate) fn decode<T>(
    status: StatusCode,
    body: &[u8],
    interaction: ProtocolInteraction,
    limits: Limits,
    success: impl FnOnce(&Json) -> Result<T, CodecError>,
) -> Result<DecodedResponse<T>, ClientResponseError> {
    let value = match json::parse(body, BodyKind::Response, limits) {
        Ok(value) => value,
        Err(_) if !status.is_success() => {
            return Err(transport_failure(status));
        }
        Err(error) => {
            return Err(ClientResponseError::invalid(
                InvalidResponseKind::InvalidSuccessBody,
                error.to_string(),
            ));
        }
    };

    match envelope_error_from_json(&value) {
        Ok(Some(error)) => {
            if status.is_success() {
                return Err(ClientResponseError::invalid(
                    InvalidResponseKind::ErrorEnvelopeOnSuccessStatus,
                    "protocol error envelope carried by a success status",
                ));
            }
            if !error.code.is_allowed_for(interaction) {
                return Err(ClientResponseError::invalid(
                    InvalidResponseKind::DisallowedProtocolCode,
                    format!(
                        "code `{}` is not allowed for {interaction:?}",
                        error_code_name(error.code)
                    ),
                ));
            }
            let expected = status_for_error(error.code);
            let status_mismatch =
                (!status_matches_error(status, error.code)).then_some(StatusMismatch {
                    actual: status,
                    expected,
                });
            Ok(DecodedResponse::ProtocolFailure {
                error,
                status_mismatch,
            })
        }
        Err(_) if !status.is_success() => Err(transport_failure(status)),
        Err(error) => Err(ClientResponseError::invalid(
            InvalidResponseKind::InvalidSuccessBody,
            error.to_string(),
        )),
        Ok(None) if status == StatusCode::OK => success(&value)
            .map(DecodedResponse::Success)
            .map_err(|error| {
                ClientResponseError::invalid(
                    InvalidResponseKind::InvalidSuccessBody,
                    error.to_string(),
                )
            }),
        Ok(None) if status.is_success() => match success(&value) {
            Ok(_) => Err(ClientResponseError::invalid(
                InvalidResponseKind::NonCanonicalSuccessStatus,
                format!("success body carried by non-canonical HTTP {status}"),
            )),
            Err(error) => Err(ClientResponseError::invalid(
                InvalidResponseKind::InvalidSuccessBody,
                error.to_string(),
            )),
        },
        Ok(None) => match success(&value) {
            Ok(_) => Err(ClientResponseError::invalid(
                InvalidResponseKind::SuccessBodyOnErrorStatus,
                "valid success body carried by a non-success status",
            )),
            Err(_) => Err(transport_failure(status)),
        },
    }
}

pub(crate) fn unsupported_content_coding(status: StatusCode) -> ClientResponseError {
    ClientResponseError::TransportBinding(TransportBindingFailure {
        status,
        kind: TransportFailureKind::UnsupportedContentCoding,
    })
}

pub(crate) fn transport_failure(status: StatusCode) -> ClientResponseError {
    let kind = match status {
        StatusCode::UNAUTHORIZED => TransportFailureKind::AuthenticationRequired,
        StatusCode::FORBIDDEN => TransportFailureKind::EndpointForbidden,
        _ => TransportFailureKind::HttpStatus,
    };
    ClientResponseError::TransportBinding(TransportBindingFailure { status, kind })
}
