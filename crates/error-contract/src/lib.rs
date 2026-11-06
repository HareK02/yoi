#![doc = include_str!("../README.md")]

pub mod codec;
pub mod compatibility;
pub mod facts;
pub mod problem;
pub mod projection;
#[cfg(feature = "json-schema")]
pub mod schema;
#[cfg(feature = "test-support")]
pub mod test_support;

pub use codec::{
    Budget, DecodeFailure, ReceivedFailure, decode_failure, encode_projected_failure,
    validate_json_input,
};
pub use facts::*;
pub use projection::*;

pub const SCHEMA_VERSION: u16 = 1;
pub const MAX_FAILURE_BYTES: usize = 32 * 1024;
pub const MAX_JSON_DEPTH: usize = 16;
pub const MAX_CONTEXTS: usize = 8;
pub const MAX_CAUSES: usize = 4;
pub const MAX_DIAGNOSTICS: usize = 8;
pub const MAX_EXPLANATION_BYTES: usize = 2 * 1024;
pub const MAX_IDENTIFIER_BYTES: usize = 128;

/// Conversion errors describe only the boundary, never the original operation's outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ContractError {
    #[error("failure exceeds the available byte budget")]
    TooLarge,
    #[error("failure exceeds the JSON depth limit")]
    TooDeep,
    #[error("invalid failure structure")]
    Invalid,
    #[error("invalid typed failure details")]
    InvalidDetails,
    #[error("failure projection unavailable")]
    ProjectionUnavailable,
    #[error("failure serialization unavailable")]
    Encoding,
}

pub fn validate_identifier(value: &str) -> Result<(), ContractError> {
    if value.is_empty() || value.len() > MAX_IDENTIFIER_BYTES || value.chars().any(char::is_control)
    {
        return Err(ContractError::Invalid);
    }
    Ok(())
}

pub fn validate_explanation(value: &str) -> Result<(), ContractError> {
    if value.is_empty() || value.len() > MAX_EXPLANATION_BYTES || value.contains('\0') {
        return Err(ContractError::Invalid);
    }
    Ok(())
}
