//! Canonical transport-independent logical types for the Web Interface Protocol.
//!
//! This crate follows the Core Model and `3-protocol.md` without assigning HTTP
//! routes, methods, headers, URL encodings, or JSON envelopes. The correspondence
//! between canonical logical concepts and this crate is direct:
//!
//! | Canonical protocol concept | Rust API |
//! | --- | --- |
//! | Object projection and ordered interface set | [`Object`] |
//! | Optional object identity / state precondition | `Object::ref` / [`Object::validator`] |
//! | Interface Descriptor | [`InterfaceDescriptor`] |
//! | Named declaration and all schema expressions | [`TypeDeclaration`] / [`TypeExpr`] |
//! | Schema-neutral primitive/composite value | [`Value`] |
//! | `observe` | [`ObserveRequest`] / [`ObserveResponse`] |
//! | `fetch_interface` | [`FetchInterfaceRequest`] / [`FetchInterfaceResponse`] |
//! | Object and interface call preconditions | [`Target`] / [`InterfaceTarget`] |
//! | `call_operation` | [`CallOperationRequest`] / [`CallOperationResponse`] |
//! | Stable Host failure taxonomy | [`ProtocolErrorCode`] / [`ProtocolError`] |
//! | Per-interaction allowed failure codes | [`ProtocolInteraction`] / [`ProtocolErrorCode::is_allowed_for`] |
//!
//! [`ProtocolErrorCode`] is a closed set of Host-returned wire failures. After a
//! Client receives a response, an invalid Object, Descriptor, operation result,
//! malformed error, or error code disallowed by [`ProtocolInteraction`] is a
//! Client-local `InvalidResponse`, not another wire error. A descriptor whose
//! `format` is not supported is a Client-local `UnsupportedDescriptorFormat`.
//!
//! For `call_operation`, [`ProtocolErrorCode::Internal`] and
//! [`ProtocolErrorCode::ResourceLimitExceeded`] guarantee that the operation did
//! not run or that all effects were rolled back. Only
//! [`ProtocolErrorCode::OperationOutcomeUnknown`] means the Host cannot make that
//! guarantee; Clients must not automatically retry it.
//!
//! Paths, object refs, interface refs, and validators remain separate concepts.
//! Interface refs use [`InterfaceReference`]; other metadata use strings/bytes in their
//! fields. In particular, object refs are optional, operation targeting uses a
//! path, and this crate defines no interaction that fetches an object by ref.
//!
//! [`Value`] contains only the canonical raw value kinds. A descriptor interprets
//! enum values as strings, entries as canonical-path strings, and unions as
//! records with a `$case` string plus an optional `value` payload. JSON uses the
//! same value tree (`Unit` is JSON null) and rejects bytes.

#![deny(missing_docs)]

mod error;
mod model;
mod validation;

pub use error::{
    NameNamespace, PathSegment, ProtocolError, ProtocolErrorCode, ProtocolInteraction,
    ValidationError, ValidationErrorKind, ValueKind,
};
pub use model::{
    CallOperationRequest, CallOperationResponse, Documentation, EnumCase, FetchInterfaceRequest,
    FetchInterfaceResponse, FieldDeclaration, InterfaceDescriptor, InterfaceReference,
    InterfaceTarget, Object, ObjectObservation, ObserveRequest, ObserveResponse,
    OperationDeclaration, ParameterDeclaration, ProtocolResult, ReturnDeclaration, Target,
    TypeDeclaration, TypeExpr, UnionCase, Value,
};

/// Validates a canonical absolute Worldspace path.
pub fn validate_path(path: &str) -> Result<(), ValidationError> {
    validation::validate_path(path)
}

/// Canonical format identifier for the first Interface Descriptor format.
pub const INTERFACE_FORMAT_V1: &str = "wip-interface/1";

/// Discriminator field used by a union's raw record value.
pub const UNION_CASE_FIELD: &str = "$case";

/// Optional payload field used by a union's raw record value.
pub const UNION_VALUE_FIELD: &str = "value";

/// Largest integer exactly interoperable with a JavaScript `Number`.
pub const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// Smallest integer exactly interoperable with a JavaScript `Number`.
pub const MIN_SAFE_INTEGER: i64 = -MAX_SAFE_INTEGER;
