//! Axum support used by routers generated with `#[api(axum)]`.
//!
//! Generated operation routers contain only transport extraction, typed service invocation, and
//! response mapping. Authentication/authorization context, body-limit policy, timeout policy, and
//! other application state remain explicit outer Axum layers. Each operation is also exposed as a
//! separate router so route-specific layers can be applied before routers are merged.

use std::{fmt, str::FromStr};

use axum::{
    Json,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Serialize;

/// Parse one contract header into its declared Rust type.
pub fn parse_header<T>(headers: &HeaderMap, name: &'static str) -> Result<T, StatusCode>
where
    T: FromStr,
{
    headers
        .get(name)
        .ok_or(StatusCode::BAD_REQUEST)?
        .to_str()
        .map_err(|_| StatusCode::BAD_REQUEST)?
        .parse()
        .map_err(|_| StatusCode::BAD_REQUEST)
}

pub fn parse_optional_header<T>(
    headers: &HeaderMap,
    name: &'static str,
) -> Result<Option<T>, StatusCode>
where
    T: FromStr,
{
    headers
        .get(name)
        .map(|value| {
            value
                .to_str()
                .map_err(|_| StatusCode::BAD_REQUEST)?
                .parse()
                .map_err(|_| StatusCode::BAD_REQUEST)
        })
        .transpose()
}

/// Opaque failure to encode one declared response header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResponseHeaderError;

impl fmt::Display for ResponseHeaderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("declared response header encoding failed")
    }
}

impl std::error::Error for ResponseHeaderError {}

/// Insert one typed declared response header without retaining its value in an error.
pub fn insert_response_header<T: fmt::Display>(
    headers: &mut HeaderMap,
    name: &'static str,
    value: &T,
) -> Result<(), ResponseHeaderError> {
    let value = value.to_string().parse().map_err(|_| ResponseHeaderError)?;
    headers.insert(name, value);
    Ok(())
}

/// Serialize a typed JSON response with the contract status.
pub fn json_response<T: Serialize>(status: StatusCode, value: T) -> Response {
    (status, Json(value)).into_response()
}

/// Emit an empty response with the contract status.
pub fn empty_response(status: StatusCode) -> Response {
    status.into_response()
}

/// Emit a transport-level rejection without a response body.
pub fn rejection(status: StatusCode) -> Response {
    status.into_response()
}

/// Convert a status constant from generated metadata.
pub fn status(code: u16) -> StatusCode {
    StatusCode::from_u16(code).expect("#[api] validates HTTP status constants")
}

/// Reexports used by generated router code.
pub mod framework {
    pub use axum::{
        Json, Router,
        body::{Body, Bytes},
        extract::{
            DefaultBodyLimit, Extension, Path, Query, State,
            rejection::{BytesRejection, JsonRejection},
        },
        http::{HeaderMap, StatusCode},
        response::Response,
        routing::{delete, get, head, options, patch, post, put},
        serve,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_header_encoding_failure_is_bounded_and_value_safe() {
        let mut headers = HeaderMap::new();
        let error = insert_response_header(&mut headers, "set-cookie", &"secret\ninvalid")
            .expect_err("invalid header bytes must fail");
        assert_eq!(
            error.to_string(),
            "declared response header encoding failed"
        );
        assert!(!format!("{error:?}").contains("secret"));
        assert!(headers.is_empty());
    }
}
