//! Reqwest transport support used by clients generated with `#[api(reqwest)]`.
//!
//! The generated builder validates the base URL before it can be stored. Callers that enforce
//! stricter egress policy (custom DNS resolution, proxy policy, TLS roots, or connection pinning)
//! can construct their own [`reqwest::Client`] and inject it with the validated [`BaseUrl`].

use std::{fmt, time::Duration};

use bytes::Bytes;
use futures::StreamExt as _;
use reqwest::{
    Client, Method, StatusCode, Url,
    header::{HeaderMap, HeaderName, HeaderValue},
};
use serde::{Serialize, de::DeserializeOwned};

/// Default timeout applied to the complete request, including response-body reads.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Default maximum response body size (1 MiB).
pub const DEFAULT_RESPONSE_BODY_LIMIT: usize = 1024 * 1024;

/// A parsed HTTP(S) base URL without credentials, query, or fragment.
#[derive(Clone, Eq, PartialEq)]
pub struct BaseUrl(Url);

impl BaseUrl {
    /// Access the validated URL.
    pub fn as_url(&self) -> &Url {
        &self.0
    }
}

impl fmt::Debug for BaseUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("BaseUrl").field(&self.0).finish()
    }
}

impl TryFrom<&str> for BaseUrl {
    type Error = ClientBuildError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Url::parse(value)
            .map_err(|_| ClientBuildError::InvalidBaseUrl)
            .and_then(Self::try_from)
    }
}

impl TryFrom<String> for BaseUrl {
    type Error = ClientBuildError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl TryFrom<Url> for BaseUrl {
    type Error = ClientBuildError;

    fn try_from(mut value: Url) -> Result<Self, Self::Error> {
        if !matches!(value.scheme(), "http" | "https")
            || value.cannot_be_a_base()
            || value.host_str().is_none()
            || !value.username().is_empty()
            || value.password().is_some()
            || value.query().is_some()
            || value.fragment().is_some()
        {
            return Err(ClientBuildError::InvalidBaseUrl);
        }
        if !value.path().ends_with('/') {
            let mut path = value.path().to_owned();
            path.push('/');
            value.set_path(&path);
        }
        Ok(Self(value))
    }
}

/// Failure while constructing a generated client.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum ClientBuildError {
    /// The base URL was not an absolute credential-free HTTP(S) URL without query or fragment.
    InvalidBaseUrl,
    /// Reqwest could not construct its default client.
    ClientConstruction,
}

impl fmt::Debug for ClientBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl fmt::Display for ClientBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidBaseUrl => "invalid HTTP client base URL",
            Self::ClientConstruction => "failed to construct HTTP client",
        })
    }
}

impl std::error::Error for ClientBuildError {}

/// Exact canonical request material supplied to a [`RequestAuthorizer`].
///
/// `body` is the same byte slice subsequently passed to Reqwest. `path_and_query` is taken from
/// the final URL after path-segment and query encoding.
pub struct AuthorizerRequest<'a> {
    pub method: &'a Method,
    pub path_and_query: &'a str,
    pub body: &'a [u8],
}

impl fmt::Debug for AuthorizerRequest<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthorizerRequest")
            .field("method", self.method)
            .field("path_and_query", &"<redacted>")
            .field("body", &"<redacted>")
            .finish()
    }
}

/// Opaque authorizer failure. Secret-bearing authorizer errors are intentionally not retained.
#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub struct AuthorizationError(());

impl AuthorizationError {
    pub const fn new() -> Self {
        Self(())
    }
}

impl fmt::Debug for AuthorizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AuthorizationError")
    }
}

impl fmt::Display for AuthorizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("request authorization failed")
    }
}

impl std::error::Error for AuthorizationError {}

/// Supplies credential headers for an exact encoded request.
pub trait RequestAuthorizer: Send + Sync {
    fn authorize(&self, request: AuthorizerRequest<'_>) -> Result<HeaderMap, AuthorizationError>;
}

/// Authorizer used by default; it adds no headers.
#[derive(Clone, Copy, Default)]
pub struct NoAuthorizer;

impl RequestAuthorizer for NoAuthorizer {
    fn authorize(&self, _request: AuthorizerRequest<'_>) -> Result<HeaderMap, AuthorizationError> {
        Ok(HeaderMap::new())
    }
}

impl fmt::Debug for NoAuthorizer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("NoAuthorizer")
    }
}

/// Builder shared by generated API-specific client builders.
pub struct ClientBuilder<A = NoAuthorizer> {
    base_url: BaseUrl,
    client: Option<Client>,
    authorizer: A,
    request_timeout: Duration,
    response_body_limit: usize,
}

impl ClientBuilder<NoAuthorizer> {
    pub fn try_new(base_url: impl AsRef<str>) -> Result<Self, ClientBuildError> {
        Ok(Self::from_base_url(BaseUrl::try_from(base_url.as_ref())?))
    }

    pub fn from_base_url(base_url: BaseUrl) -> Self {
        Self {
            base_url,
            client: None,
            authorizer: NoAuthorizer,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            response_body_limit: DEFAULT_RESPONSE_BODY_LIMIT,
        }
    }
}

impl<A> ClientBuilder<A> {
    pub fn client(mut self, client: Client) -> Self {
        self.client = Some(client);
        self
    }

    pub fn authorizer<B>(self, authorizer: B) -> ClientBuilder<B> {
        ClientBuilder {
            base_url: self.base_url,
            client: self.client,
            authorizer,
            request_timeout: self.request_timeout,
            response_body_limit: self.response_body_limit,
        }
    }

    pub fn request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    pub fn response_body_limit(mut self, limit: usize) -> Self {
        self.response_body_limit = limit;
        self
    }

    pub fn build(self) -> Result<ClientCore<A>, ClientBuildError> {
        let client = match self.client {
            Some(client) => client,
            None => Client::builder()
                .build()
                .map_err(|_| ClientBuildError::ClientConstruction)?,
        };
        Ok(ClientCore {
            base_url: self.base_url,
            client,
            authorizer: self.authorizer,
            request_timeout: self.request_timeout,
            response_body_limit: self.response_body_limit,
        })
    }
}

impl<A> fmt::Debug for ClientBuilder<A> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClientBuilder")
            .field("base_url", &self.base_url)
            .field("client", &"<opaque>")
            .field("authorizer", &"<redacted>")
            .field("request_timeout", &self.request_timeout)
            .field("response_body_limit", &self.response_body_limit)
            .finish()
    }
}

/// Framework support held by each generated API client.
pub struct ClientCore<A = NoAuthorizer> {
    base_url: BaseUrl,
    client: Client,
    authorizer: A,
    request_timeout: Duration,
    response_body_limit: usize,
}

impl<A: Clone> Clone for ClientCore<A> {
    fn clone(&self) -> Self {
        Self {
            base_url: self.base_url.clone(),
            client: self.client.clone(),
            authorizer: self.authorizer.clone(),
            request_timeout: self.request_timeout,
            response_body_limit: self.response_body_limit,
        }
    }
}

impl<A> fmt::Debug for ClientCore<A> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClientCore")
            .field("base_url", &self.base_url)
            .field("client", &"<opaque>")
            .field("authorizer", &"<redacted>")
            .field("request_timeout", &self.request_timeout)
            .field("response_body_limit", &self.response_body_limit)
            .finish()
    }
}

impl<A> ClientCore<A> {
    pub fn from_parts(
        base_url: BaseUrl,
        client: Client,
        authorizer: A,
        request_timeout: Duration,
        response_body_limit: usize,
    ) -> Self {
        Self {
            base_url,
            client,
            authorizer,
            request_timeout,
            response_body_limit,
        }
    }

    /// Build an endpoint by appending already-separated, unencoded path segments.
    pub fn endpoint(&self, segments: &[String]) -> Result<Url, ClientFailure> {
        let mut url = self.base_url.0.clone();
        let mut path = url
            .path_segments_mut()
            .map_err(|_| ClientFailure::RequestEncoding)?;
        path.pop_if_empty();
        for segment in segments {
            path.push(segment);
        }
        drop(path);
        Ok(url)
    }

    pub fn response_body_limit(&self) -> usize {
        self.response_body_limit
    }
}

/// A prepared request body whose bytes are authorized and transmitted without further encoding.
pub struct EncodedBody {
    bytes: Bytes,
    content_type: &'static str,
}

impl EncodedBody {
    fn json(bytes: Vec<u8>) -> Self {
        Self {
            bytes: bytes.into(),
            content_type: "application/json",
        }
    }

    fn binary(body: crate::BinaryBody) -> Self {
        Self {
            bytes: body.into_bytes(),
            content_type: "application/octet-stream",
        }
    }
}

impl fmt::Debug for EncodedBody {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EncodedBody")
            .field("bytes", &"<redacted>")
            .field("content_type", &self.content_type)
            .finish()
    }
}

impl<A: RequestAuthorizer> ClientCore<A> {
    pub async fn send(
        &self,
        method: Method,
        url: Url,
        mut headers: HeaderMap,
        body: Option<EncodedBody>,
    ) -> Result<ReceivedResponse, ClientFailure> {
        let body_bytes = body
            .as_ref()
            .map(|body| body.bytes.as_ref())
            .unwrap_or_default();
        let path_and_query = &url[url::Position::BeforePath..url::Position::AfterQuery];
        let authorization = self
            .authorizer
            .authorize(AuthorizerRequest {
                method: &method,
                path_and_query,
                body: body_bytes,
            })
            .map_err(|_| ClientFailure::Authorization)?;
        headers.extend(authorization);
        if let Some(body) = body.as_ref() {
            headers.insert(
                reqwest::header::CONTENT_TYPE,
                HeaderValue::from_static(body.content_type),
            );
        }

        let mut request = self
            .client
            .request(method, url)
            .headers(headers)
            .timeout(self.request_timeout);
        if let Some(body) = body {
            request = request.body(body.bytes);
        }
        let response = request.send().await.map_err(|error| {
            if error.is_timeout() {
                ClientFailure::Timeout
            } else if error.is_connect() {
                ClientFailure::Connect
            } else {
                ClientFailure::Transport
            }
        })?;
        let status = response.status();
        let headers = response.headers().clone();
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| {
                if error.is_timeout() {
                    ClientFailure::Timeout
                } else {
                    ClientFailure::Transport
                }
            })?;
            let Some(size) = bytes.len().checked_add(chunk.len()) else {
                return Err(ClientFailure::ResponseTooLarge {
                    limit: self.response_body_limit,
                });
            };
            if size > self.response_body_limit {
                return Err(ClientFailure::ResponseTooLarge {
                    limit: self.response_body_limit,
                });
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(ReceivedResponse {
            status,
            headers,
            bytes,
        })
    }
}

/// Encode a serializable query DTO and append its pairs without changing existing pairs.
pub fn append_query<T: Serialize + ?Sized>(url: &mut Url, value: &T) -> Result<(), ClientFailure> {
    let encoded = serde_urlencoded::to_string(value).map_err(|_| ClientFailure::RequestEncoding)?;
    if encoded.is_empty() {
        return Ok(());
    }
    let combined = match url.query() {
        Some(existing) if !existing.is_empty() => format!("{existing}&{encoded}"),
        _ => encoded,
    };
    url.set_query(Some(&combined));
    Ok(())
}

/// Insert a string-form header value.
pub fn insert_header<T: fmt::Display>(
    headers: &mut HeaderMap,
    name: &str,
    value: &T,
) -> Result<(), ClientFailure> {
    let name =
        HeaderName::from_bytes(name.as_bytes()).map_err(|_| ClientFailure::RequestEncoding)?;
    let value =
        HeaderValue::from_str(&value.to_string()).map_err(|_| ClientFailure::RequestEncoding)?;
    headers.insert(name, value);
    Ok(())
}

/// Insert an optional string-form header value when present.
pub fn insert_optional_header<T: fmt::Display>(
    headers: &mut HeaderMap,
    name: &str,
    value: &Option<T>,
) -> Result<(), ClientFailure> {
    match value {
        Some(value) => insert_header(headers, name, value),
        None => Ok(()),
    }
}

/// Serialize a JSON request body once. These exact bytes are authorized and transmitted.
pub fn encode_json<T: Serialize + ?Sized>(value: &T) -> Result<EncodedBody, ClientFailure> {
    serde_json::to_vec(value)
        .map(EncodedBody::json)
        .map_err(|_| ClientFailure::RequestEncoding)
}

/// Move an explicit binary request body into the exact authorized/transmitted representation.
pub fn encode_binary(value: crate::BinaryBody) -> EncodedBody {
    EncodedBody::binary(value)
}

/// Response received under the configured byte limit. Its content is always redacted in Debug.
pub struct ReceivedResponse {
    status: StatusCode,
    headers: HeaderMap,
    bytes: Vec<u8>,
}

impl ReceivedResponse {
    pub fn status(&self) -> StatusCode {
        self.status
    }

    pub fn parse_header<T>(&self, name: &'static str) -> Result<T, ClientFailure>
    where
        T: std::str::FromStr,
    {
        let mut values = self.headers.get_all(name).iter();
        let value = values
            .next()
            .ok_or(ClientFailure::MissingResponseHeader { name })?;
        if values.next().is_some() {
            return Err(ClientFailure::DuplicateResponseHeader { name });
        }
        value
            .to_str()
            .map_err(|_| ClientFailure::InvalidResponseHeader { name })?
            .parse()
            .map_err(|_| ClientFailure::InvalidResponseHeader { name })
    }

    pub fn decode_json<T: DeserializeOwned>(&self, kind: DecodeKind) -> Result<T, ClientFailure> {
        serde_json::from_slice(&self.bytes).map_err(|_| ClientFailure::Decode { kind })
    }

    pub fn require_empty(&self) -> Result<(), ClientFailure> {
        if self.bytes.is_empty() {
            Ok(())
        } else {
            Err(ClientFailure::UnexpectedBody)
        }
    }
}

impl fmt::Debug for ReceivedResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReceivedResponse")
            .field("status", &self.status)
            .field("headers", &"<redacted>")
            .field("bytes", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeKind {
    Success,
    PublicError,
}

/// Non-public-error failure from a generated client.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum ClientFailure {
    Authorization,
    RequestEncoding,
    Connect,
    Transport,
    Timeout,
    ResponseTooLarge { limit: usize },
    UnexpectedStatus { expected: u16, actual: u16 },
    ErrorResponseDecode { status: u16 },
    Decode { kind: DecodeKind },
    MissingResponseHeader { name: &'static str },
    DuplicateResponseHeader { name: &'static str },
    InvalidResponseHeader { name: &'static str },
    UnexpectedBody,
}

impl fmt::Debug for ClientFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl fmt::Display for ClientFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Authorization => formatter.write_str("request authorization failed"),
            Self::RequestEncoding => formatter.write_str("request encoding failed"),
            Self::Connect => formatter.write_str("HTTP connection failed"),
            Self::Transport => formatter.write_str("HTTP transport failed"),
            Self::Timeout => formatter.write_str("HTTP request timed out"),
            Self::ResponseTooLarge { limit } => {
                write!(formatter, "response body exceeded {limit} bytes")
            }
            Self::UnexpectedStatus { expected, actual } => {
                write!(
                    formatter,
                    "expected HTTP status {expected}, received {actual}"
                )
            }
            Self::ErrorResponseDecode { status } => {
                write!(formatter, "HTTP {status} error response JSON was malformed")
            }
            Self::Decode {
                kind: DecodeKind::Success,
            } => formatter.write_str("success response JSON was malformed"),
            Self::Decode {
                kind: DecodeKind::PublicError,
            } => formatter.write_str("public error response JSON was malformed"),
            Self::MissingResponseHeader { name } => {
                write!(formatter, "response header `{name}` was missing")
            }
            Self::DuplicateResponseHeader { name } => {
                write!(
                    formatter,
                    "response header `{name}` occurred more than once"
                )
            }
            Self::InvalidResponseHeader { name } => {
                write!(formatter, "response header `{name}` was invalid")
            }
            Self::UnexpectedBody => formatter.write_str("empty response contained a body"),
        }
    }
}

impl std::error::Error for ClientFailure {}

/// Typed error returned by generated client methods.
pub enum ClientError<E> {
    Failure(ClientFailure),
    Public { status: StatusCode, error: E },
}

impl<E> ClientError<E> {
    pub fn failure(failure: ClientFailure) -> Self {
        Self::Failure(failure)
    }

    pub fn public(status: StatusCode, error: E) -> Self {
        Self::Public { status, error }
    }

    pub fn into_public_error(self) -> Option<E> {
        match self {
            Self::Public { error, .. } => Some(error),
            Self::Failure(_) => None,
        }
    }
}

impl<E> From<ClientFailure> for ClientError<E> {
    fn from(value: ClientFailure) -> Self {
        Self::Failure(value)
    }
}

impl<E> fmt::Debug for ClientError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failure(failure) => formatter.debug_tuple("Failure").field(failure).finish(),
            Self::Public { status, .. } => formatter
                .debug_struct("Public")
                .field("status", status)
                .field("error", &"<redacted>")
                .finish(),
        }
    }
}

impl<E> fmt::Display for ClientError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failure(failure) => fmt::Display::fmt(failure, formatter),
            Self::Public { status, .. } => write!(formatter, "public HTTP error {status}"),
        }
    }
}

impl<E> std::error::Error for ClientError<E> {}

/// Reexports used by generated code and advanced client construction.
pub mod framework {
    pub use reqwest::{Client, Method, StatusCode, Url, header};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bodyless_response_validation_rejects_retained_bytes_without_exposing_them() {
        let response = ReceivedResponse {
            status: StatusCode::NOT_MODIFIED,
            headers: HeaderMap::new(),
            bytes: b"sensitive-body".to_vec(),
        };
        assert_eq!(response.require_empty(), Err(ClientFailure::UnexpectedBody));
        assert!(!format!("{response:?}").contains("sensitive-body"));
    }
}
