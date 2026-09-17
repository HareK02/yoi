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
//! `204`. A public error inferred from `Result<T, E>` defaults to status `400`.
//!
//! Arguments are classified with `#[body]`, `#[query]`, `#[header]`, or `#[path]`. An
//! unannotated argument whose Rust name occurs in the route template is inferred as a path
//! argument. Header attributes may carry a wire name, as in `#[header("x-request-id")]`.
//! Exactly one JSON body is allowed. JSON request, response, and error bodies must be named
//! Rust types; tuples, references, arrays, and other anonymous structural types are rejected.
//!
//! # Generated names
//!
//! For `WidgetApi`, the macro emits `WidgetApiMetadata`, which implements [`ApiContract`],
//! plus a `widget_api_operations` module. That module contains one marker per Rust method
//! (`Create`, `Get`, ...), each implementing [`Operation`] and connecting metadata to concrete
//! body types at compile time. The `ApiContract::OPERATIONS` inventory is sorted by operation
//! ID, so its ordering is independent of source method order.
//!
//! Only empty and JSON bodies are accepted by this first contract. [`WireKind`] reserves
//! explicit variants for future transport work; accepting one requires a deliberate macro and
//! adapter change rather than silently treating it as JSON.

pub use api_macros_impl::api;

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

/// Marker used when an operation has no request, response, or public error body.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum NoBody {}
