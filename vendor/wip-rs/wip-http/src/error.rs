use std::error::Error;
use std::fmt::{self, Display, Formatter};

/// Caller-selected hard limits applied before or while decoding JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Largest accepted request body in bytes.
    pub(crate) max_request_bytes: usize,
    /// Largest accepted response body in bytes.
    pub(crate) max_response_bytes: usize,
    /// Largest accepted JSON container nesting depth.
    pub(crate) max_nesting: usize,
}

impl Limits {
    /// Creates a nonzero limit set.
    pub fn new(
        max_request_bytes: usize,
        max_response_bytes: usize,
        max_nesting: usize,
    ) -> Result<Self, LimitConfigurationError> {
        if max_request_bytes == 0 {
            return Err(LimitConfigurationError::ZeroRequestBytes);
        }
        if max_response_bytes == 0 {
            return Err(LimitConfigurationError::ZeroResponseBytes);
        }
        if max_nesting == 0 {
            return Err(LimitConfigurationError::ZeroNesting);
        }
        Ok(Self {
            max_request_bytes,
            max_response_bytes,
            max_nesting,
        })
    }

    /// Returns the request body byte limit.
    #[must_use]
    pub const fn max_request_bytes(self) -> usize {
        self.max_request_bytes
    }

    /// Returns the response body byte limit.
    #[must_use]
    pub const fn max_response_bytes(self) -> usize {
        self.max_response_bytes
    }

    /// Returns the JSON container nesting limit.
    #[must_use]
    pub const fn max_nesting(self) -> usize {
        self.max_nesting
    }
}

/// Why a [`Limits`] configuration is not bounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitConfigurationError {
    /// Request-byte limit was zero.
    ZeroRequestBytes,
    /// Response-byte limit was zero.
    ZeroResponseBytes,
    /// Nesting limit was zero.
    ZeroNesting,
}

impl Display for LimitConfigurationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ZeroRequestBytes => "request byte limit must be nonzero",
            Self::ZeroResponseBytes => "response byte limit must be nonzero",
            Self::ZeroNesting => "nesting limit must be nonzero",
        })
    }
}

impl Error for LimitConfigurationError {}

/// Whether a body is being decoded as a request or response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyKind {
    /// An inbound server request.
    Request,
    /// An inbound client response.
    Response,
}

/// A malformed or locally disallowed wire representation.
#[derive(Debug)]
pub enum CodecError {
    /// The body exceeds its configured byte limit.
    BodyTooLarge {
        /// Actual body size.
        actual: usize,
        /// Configured maximum.
        maximum: usize,
    },
    /// JSON syntax is invalid.
    InvalidJson(serde_json::Error),
    /// A JSON container exceeds the configured nesting depth.
    NestingTooDeep {
        /// Configured maximum.
        maximum: usize,
    },
    /// A required field is absent.
    MissingField {
        /// Field name.
        field: String,
    },
    /// An undeclared field is present.
    UnknownField {
        /// Field name.
        field: String,
    },
    /// A field has an invalid JSON type or semantic representation.
    InvalidField {
        /// Field name or value location.
        field: String,
        /// Stable human-readable reason.
        reason: String,
    },
    /// An optional field was represented by JSON null rather than omission.
    NullOptionalField {
        /// Field name.
        field: String,
    },
    /// Base64 is not canonical RFC 4648 Section 4 encoding.
    InvalidBase64 {
        /// Field name or value location.
        field: String,
    },
    /// Logical protocol validation rejected the decoded representation.
    ProtocolValidation(wip_protocol::ValidationError),
}

impl Display for CodecError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::BodyTooLarge { actual, maximum } => {
                write!(formatter, "body has {actual} bytes; maximum is {maximum}")
            }
            Self::InvalidJson(error) => write!(formatter, "invalid JSON: {error}"),
            Self::NestingTooDeep { maximum } => {
                write!(formatter, "JSON nesting exceeds maximum {maximum}")
            }
            Self::MissingField { field } => write!(formatter, "missing field `{field}`"),
            Self::UnknownField { field } => write!(formatter, "unknown field `{field}`"),
            Self::InvalidField { field, reason } => {
                write!(formatter, "invalid field `{field}`: {reason}")
            }
            Self::NullOptionalField { field } => {
                write!(
                    formatter,
                    "optional field `{field}` must be omitted instead of null"
                )
            }
            Self::InvalidBase64 { field } => {
                write!(formatter, "field `{field}` is not canonical base64")
            }
            Self::ProtocolValidation(error) => error.fmt(formatter),
        }
    }
}

impl Error for CodecError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidJson(error) => Some(error),
            Self::ProtocolValidation(error) => Some(error),
            _ => None,
        }
    }
}

impl From<wip_protocol::ValidationError> for CodecError {
    fn from(value: wip_protocol::ValidationError) -> Self {
        Self::ProtocolValidation(value)
    }
}
