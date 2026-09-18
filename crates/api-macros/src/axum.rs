//! Axum support used by routers generated with `#[api(axum)]`.
//!
//! Generated operation routers contain only transport extraction, typed service invocation, and
//! response mapping. Authentication/authorization context, body-limit policy, timeout policy, and
//! other application state remain explicit outer Axum layers. Each operation is also exposed as a
//! separate router so route-specific layers can be applied before routers are merged.

use std::str::FromStr;

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
        extract::{Extension, Path, Query, State, rejection::JsonRejection},
        http::{HeaderMap, StatusCode},
        response::Response,
        routing::{delete, get, head, options, patch, post, put},
        serve,
    };
}
