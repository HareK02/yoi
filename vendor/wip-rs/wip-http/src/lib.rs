//! Framework-independent standard WIP over HTTP v1 binding.
//!
//! This crate owns endpoint and route derivation, UTF-8 JSON encoding, canonical
//! success and error response handling, HTTP status mapping, descriptor-guided
//! operation-value conversion. It deliberately performs no I/O and depends on no
//! server framework, TLS implementation, filesystem projection, client cache, or
//! presentation layer. URL schemes, TLS, certificates, authentication mechanisms,
//! proxies, and caches are selected by the HTTP implementation and deployment
//! rather than by WIP conformance.
//!
//! Logical values remain owned by [`wip_protocol`]. Validators and `Bytes` are
//! encoded in JSON bodies using canonical RFC 4648 Section 4 base64; this crate
//! does not infer WIP validator semantics from `ETag` or `If-Match`.
//!
//! Interface references use exact JSON objects with required `scope` and `name`
//! strings in every position. Legacy strings and display shorthand are rejected.
//! Scope is a canonical absolute Worldspace path, not an HTTP endpoint prefix.
//! Optional `scope_ref` metadata on fetch responses and call interface targets is
//! an opaque string, never base64; absent fields are omitted and null is rejected.
//! Call metadata validates reference shape without resolving scope ancestry or
//! identity, which remain Host checks with `InterfaceMismatch` precedence.
//!
//! ```
//! use wip_http::{Endpoint, Limits, encode_observe_request};
//! use wip_protocol::ObserveRequest;
//!
//! let endpoint = Endpoint::parse("http://example.test/api/wip")?;
//! let limits = Limits::new(64 * 1024, 1024 * 1024, 64)?;
//! let request = encode_observe_request(
//!     &endpoint,
//!     &ObserveRequest { path: "/items/123".into(), depth: 0 },
//!     limits,
//! )?;
//! assert_eq!(request.uri(), "http://example.test/api/wip/v1/observe");
//! assert_eq!(request.method(), http::Method::POST);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![deny(missing_docs)]

mod codec;
mod endpoint;
mod error;
mod json;
mod response;

pub use codec::{
    CallOperationMetadata, EncodeHttpError, RequestMetadataError, decode_call_operation_metadata,
    decode_call_operation_request, decode_call_operation_response, decode_fetch_interface_request,
    decode_fetch_interface_response, decode_observe_request, decode_observe_response,
    encode_call_operation_request, encode_call_operation_response, encode_fetch_interface_request,
    encode_fetch_interface_response, encode_observe_request, encode_observe_response,
    encode_protocol_error_response, is_json_content_type, validate_http_request,
};
pub use endpoint::{Endpoint, EndpointError, JSON_CONTENT_TYPE, Route};
pub use error::{BodyKind, CodecError, LimitConfigurationError, Limits};
pub use response::{
    ClientResponseError, DecodedResponse, InvalidResponseKind, StatusMismatch,
    TransportBindingFailure, TransportFailureKind, error_code_name, status_for_error,
};

/// Framework-neutral HTTP types used by this binding's public API.
pub use http;
/// URL type used by [`Endpoint`].
pub use url;
/// Transport-independent logical protocol model used by this binding.
pub use wip_protocol;
