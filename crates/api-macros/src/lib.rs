//! Shared API trait declarations for Yoi services.
//!
//! `#[api]` turns a public trait into a single, transport-neutral API definition. The
//! original trait remains usable by Rust code; generated metadata and zero-sized operation
//! markers give adapters and generators a stable view of the same declaration.
//!
//! # Declaration syntax
//!
//! ```
//! # mod example {
//! use api_macros::api;
//!
//! pub struct CreateWidget;
//! pub struct Widget;
//! pub struct PublicError;
//!
//! #[api]
//! pub trait WidgetApi {
//!     #[post(
//!         "/widgets",
//!         operation_id = "widgets.create",
//!         status = 201,
//!         error_status = 422
//!     )]
//!     async fn create(
//!         &self,
//!         #[body] request: CreateWidget,
//!         #[header("x-request-id")] request_id: String,
//!     ) -> Result<Widget, PublicError>;
//!
//!     #[get("/widgets/{widget_id}", operation_id = "widgets.get")]
//!     async fn get(&self, widget_id: u64) -> Result<Widget, PublicError>;
//! }
//! # }
//! ```
//!
//! Supported route attributes are `get`, `post`, `put`, `patch`, `delete`, `head`, and
//! `options`. The first argument is a path template. `operation_id` defaults to the Rust
//! method name but should be set explicitly for contracts which must remain stable while Rust
//! names evolve. Success status defaults to `200`, except an empty (`()`) response defaults to
//! `204`. `alternate_status` opts a JSON response into [`HttpSuccess`] when two successful HTTP
//! outcomes share one schema. A public error inferred from `Result<T, E>` defaults to status `400`;
//! `additional_error_statuses` publishes the same typed error schema for other declared outcomes.
//! `bearer_auth = true` and `browser_auth = true` attach standard bearer and browser-session cookie
//! security schemes. Body operations may opt into typed Axum rejection normalization with
//! typed Axum rejection normalization with `normalize_body_errors = true` and [`HttpRequestError`].
//!
//! Arguments are classified with `#[body]`, `#[query]`, `#[header]`, `#[path]`, or `#[extension]`.
//! Extension values are trusted server-local Axum context: generated clients and OpenAPI omit them.
//! An unannotated argument whose Rust name occurs in the route template is inferred as a path
//! argument. Header attributes may carry a wire name, as in `#[header("x-request-id")]`.
//! Exactly one JSON body is allowed. JSON request, response, and error bodies must be named
//! Rust types; tuples, references, arrays, and other anonymous structural types are rejected.
//! `openapi = false` explicitly excludes an operation whose wire body cannot yet satisfy the
//! strict OpenAPI schema boundary; Reqwest and Axum adapters are still generated.
//!
//! # Generated names
//!
//! For `WidgetApi`, the macro emits `WidgetApiMetadata`, which implements [`ApiContract`],
//! plus a `widget_api_operations` module. That module contains one marker per Rust method
//! (`Create`, `Get`, ...), each implementing [`Operation`] and connecting metadata to concrete
//! body types at compile time. Unusual method names which have no valid PascalCase form use the
//! deterministic `Operation<utf8-hex>` fallback. The `ApiContract::OPERATIONS` inventory is
//! sorted by operation ID, so its ordering is independent of source method order.
//!
//! Only empty and JSON bodies are accepted by this first contract. [`WireKind`] reserves
//! explicit variants for future transport work; accepting one requires a deliberate macro and
//! adapter change rather than silently treating it as JSON.
//!
//! # OpenAPI 3.1 export
//!
//! With the `openapi` feature, `#[api(openapi)]` emits a `<trait_name>_openapi` function from the
//! same normalized operation model used by the HTTP adapters. The caller supplies title, version,
//! and an opaque source digest through [`openapi::OpenApiInfo`], then either serves
//! [`openapi::OpenApiDocument::to_json`] or writes exactly those canonical bytes with
//! [`openapi::OpenApiDocument::write_json`] from a CLI. Documents contain no server URLs,
//! environment names, timestamps, or other deployment topology.
//!
//! Every exposed wire type must implement [`openapi::OpenApiSchema`] as an explicit assertion that
//! its `schemars::JsonSchema` contract matches Serde serialization. The trait's naming hook is the
//! public component-name contract. Different schemas claiming one name, unsafe-width integer
//! schemas, and ambiguous non-null `anyOf`/untagged representations fail closed. Use bounded
//! integers (for example `u32`) for JSON numeric fields, and model optionality separately from
//! nullability through object `required` membership and nullable schemas. A normalized
//! `#[header("authorization")]` string (or optional string) is projected as an HTTP bearer
//! security scheme and operation requirement rather than as a raw header parameter.
//!
//! # Optional HTTP adapters
//!
//! `#[api(reqwest)]` generates `TraitNameClient` and enables a typed Reqwest client when this
//! crate's `reqwest` feature is enabled. `#[api(axum)]` generates `TraitNameAxum` plus one public
//! router per operation when the `axum` feature is enabled. `#[api(reqwest, axum)]` emits both
//! from the same normalized definition. A plain `#[api]` remains framework-independent and pulls
//! neither framework into DTO/contract-only builds.
//!
//! Generated clients use a fallible `try_new` constructor and builder. The builder validates an
//! absolute credential-free HTTP(S) base URL and configures request timeout and response byte
//! limit. A prevalidated [`reqwest::BaseUrl`] and policy-configured `reqwest::framework::Client`
//! may be injected instead. Request authorizers receive the final method, encoded path/query, and
//! exact serialized body bytes; credential headers and all body content are redacted from normal
//! diagnostics.
//!
//! Generated Axum adapters keep authentication, authorization, application state, body limits,
//! and timeout layers outside the adapter. Callers may layer the complete router or an individual
//! generated operation router before merging it into their application.

extern crate self as api_macros;

pub use api_macros_impl::api;

#[cfg(feature = "axum")]
pub mod axum;
#[cfg(feature = "openapi")]
pub mod openapi;
#[cfg(feature = "reqwest")]
pub mod reqwest;

/// A transport-neutral HTTP method.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
}

/// Where an operation argument is encoded on the wire.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ParameterLocation {
    Path,
    Query,
    Header,
    Body,
}

/// Body encodings understood or explicitly reserved by the API contract.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum WireKind {
    Empty,
    Json,
    Streaming,
    Multipart,
    Binary,
}

/// Stable metadata for one Rust method argument.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ParameterMetadata {
    pub rust_name: &'static str,
    pub wire_name: &'static str,
    pub location: ParameterLocation,
}

/// Stable metadata for a request or response body.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BodyMetadata {
    pub wire_kind: WireKind,
}

/// Stable metadata for one HTTP response.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ResponseMetadata {
    pub status: u16,
    pub body: BodyMetadata,
}

/// Complete transport metadata for one operation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct OperationMetadata {
    pub operation_id: &'static str,
    pub method: HttpMethod,
    pub path: &'static str,
    pub parameters: &'static [ParameterMetadata],
    pub request_body: BodyMetadata,
    pub response: ResponseMetadata,
    pub error_response: Option<ResponseMetadata>,
}

/// Deterministic operation inventory emitted for an `#[api]` trait.
pub trait ApiContract {
    const OPERATIONS: &'static [OperationMetadata];
}

/// Compile-time connection between operation metadata and its concrete body types.
pub trait Operation {
    /// Tuple of all non-receiver parameter types in declaration order.
    type Parameters;
    /// JSON request body type, or [`NoBody`].
    type RequestBody;
    /// JSON success response body type, or [`NoBody`].
    type ResponseBody;
    /// JSON public error body type, or [`NoBody`].
    type ErrorBody;

    const METADATA: OperationMetadata;
}

/// Public JSON error that can select its concrete HTTP status at the server boundary.
///
/// `error_status` remains the contract's default/documented status. Implementations may return a
/// different 4xx/5xx status for one named error shape, allowing a generated client to preserve a
/// service's typed 401/403/404/409 responses without introducing duplicate route handlers.
pub trait HttpError {
    fn status_code(&self) -> u16;
}

/// Successful JSON response that selects one of an operation's declared success statuses.
///
/// Operations opt in with `alternate_status`; ordinary one-status operations remain fully static.
pub trait HttpSuccess {
    fn status_code(&self) -> u16;
}

/// Public error type capable of normalizing Axum request-body rejections.
pub trait HttpRequestError: HttpError {
    fn from_request_rejection(status: u16, message: String) -> Self;
}

/// Marker used when an operation has no request, response, or public error body.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum NoBody {}
