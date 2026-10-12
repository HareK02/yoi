#![doc = include_str!("../README.md")]

extern crate self as api_macros;

/// Normalize a public trait into a transport-independent, typed API contract.
///
/// Preserve the original Rust trait and generate operation markers and metadata from the
/// same declaration. HTTP adapters and OpenAPI use the same normalized result, avoiding
/// manual synchronization of separate route lists. Sort the inventory by operation ID so
/// declaration reordering does not change the external contract order.
///
/// Validate request/response wire shapes at declaration time; unsupported types and duplicate
/// routes are compile errors. Exclude server-local extensions from clients and OpenAPI;
/// leave authentication, authorization, body limits, and timeouts to the enclosing application.
/// Declarations without adapters do not depend on an HTTP framework.
///
/// This declaration checks that a Rust trait and typed operation markers can be defined together.
///
/// ```
/// # mod example {
/// use api_macros::api;
///
/// pub struct CreateWidget;
/// pub struct Widget;
/// pub struct PublicError;
///
/// #[api]
/// pub trait WidgetApi {
///     #[post(
///         "/widgets",
///         operation_id = "widgets.create",
///         status = 201,
///         error_status = 422
///     )]
///     async fn create(
///         &self,
///         #[body] request: CreateWidget,
///         #[header("x-request-id")] request_id: String,
///     ) -> Result<Widget, PublicError>;
///
///     #[get("/widgets/{widget_id}", operation_id = "widgets.get")]
///     async fn get(&self, widget_id: u64) -> Result<Widget, PublicError>;
/// }
/// # }
/// ```
///
/// WebSocket associated types are declarations only: remove them from the output trait
/// and turn them into [`WebSocketOperation`] markers. They are not unary HTTP operations
/// and generate neither client methods nor OpenAPI paths.
///
/// ```
/// # mod websocket_example {
/// # use api_macros::api;
/// # pub struct ClientFrame;
/// # pub struct ServerFrame;
/// #[api]
/// pub trait EventApi {
///     #[websocket(
///         "/events/{stream_id}",
///         operation_id = "events.stream",
///         method = GET,
///         client_to_server = ClientFrame,
///         server_to_client = ServerFrame,
///         path_parameters = [stream_id: u64]
///     )]
///     type EventStream;
/// }
/// # }
/// ```
pub use api_macros_impl::api;

#[cfg(feature = "axum")]
pub mod axum;
#[cfg(feature = "openapi")]
pub mod openapi;
#[cfg(feature = "reqwest")]
pub mod reqwest;

/// Framework-neutral owned bytes used by explicit `#[binary]` requests and binary success responses.
///
/// The wrapper preserves the exact byte sequence while keeping payload content out of `Debug`
/// output. Its `bytes::Bytes` storage lets generated Axum adapters transfer an extracted body
/// without reserialization and lets generated Reqwest clients authorize and send the same storage.
#[derive(Clone, Default, Eq, Hash, PartialEq)]
pub struct BinaryBody(bytes::Bytes);

impl BinaryBody {
    pub const fn new(bytes: bytes::Bytes) -> Self {
        Self(bytes)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn into_bytes(self) -> bytes::Bytes {
        self.0
    }
}

impl AsRef<[u8]> for BinaryBody {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl std::ops::Deref for BinaryBody {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl From<bytes::Bytes> for BinaryBody {
    fn from(value: bytes::Bytes) -> Self {
        Self(value)
    }
}

impl From<Vec<u8>> for BinaryBody {
    fn from(value: Vec<u8>) -> Self {
        Self(value.into())
    }
}

impl From<&'static [u8]> for BinaryBody {
    fn from(value: &'static [u8]) -> Self {
        Self(bytes::Bytes::from_static(value))
    }
}

impl std::fmt::Debug for BinaryBody {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("BinaryBody(<redacted>)")
    }
}

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

/// Stable metadata for one operation argument.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ParameterMetadata {
    pub rust_name: &'static str,
    pub rust_type: &'static str,
    pub wire_name: &'static str,
    pub location: ParameterLocation,
}

/// Transport semantics declared by one operation.
///
/// WebSocket frame directions remain distinct even when both directions use the same Rust type.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TransportMetadata {
    Http,
    WebSocket {
        client_to_server_frame: &'static str,
        server_to_client_frame: &'static str,
    },
}

/// Stable metadata for a request or response body.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BodyMetadata {
    pub wire_kind: WireKind,
}

/// Stable metadata for one explicitly declared response header.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ResponseHeaderMetadata {
    /// Canonical wire name declared in the API contract.
    pub wire_name: &'static str,
    /// Rust type spelling declared alongside the wire name.
    pub rust_type: &'static str,
}

/// Stable metadata for one HTTP response.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ResponseMetadata {
    pub status: u16,
    pub body: BodyMetadata,
    pub headers: &'static [ResponseHeaderMetadata],
}

/// Complete transport metadata for one operation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct OperationMetadata {
    pub operation_id: &'static str,
    pub method: HttpMethod,
    pub path: &'static str,
    pub transport: TransportMetadata,
    pub parameters: &'static [ParameterMetadata],
    pub request_body: BodyMetadata,
    /// Every declared successful response, including status-specific body and header shape.
    pub success_responses: &'static [ResponseMetadata],
    /// Every declared public-error status. Error responses currently have no declared headers.
    pub error_responses: &'static [ResponseMetadata],
}

/// Deterministic operation inventory emitted for an `#[api]` trait.
pub trait ApiContract {
    const OPERATIONS: &'static [OperationMetadata];
}

/// Compile-time connection between operation metadata and its concrete body types.
pub trait Operation {
    /// Tuple of all non-receiver parameter types in declaration order.
    type Parameters;
    /// Typed JSON or binary request body, or [`NoBody`].
    type RequestBody;
    /// JSON or binary success body type, generated declared-response result, or [`NoBody`].
    type ResponseBody;
    /// JSON public error body type, or [`NoBody`].
    type ErrorBody;

    const METADATA: OperationMetadata;
}

/// Compile-time connection between a WebSocket route and its directional frame authorities.
///
/// This is intentionally separate from [`Operation`]: WebSocket routes do not have unary request,
/// response, or error bodies and therefore cannot be passed to HTTP adapter surfaces.
pub trait WebSocketOperation {
    /// Tuple of path parameter types in path-template order.
    type PathParameters;
    /// Frame received by the server from the connected client.
    type ClientToServerFrame;
    /// Frame sent by the server to the connected client.
    type ServerToClientFrame;

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
