use std::error::Error;
use std::fmt::{self, Display, Formatter};

use http::header::{ACCEPT, CONTENT_ENCODING, CONTENT_TYPE};
use http::{HeaderMap, Method, Request, Response, StatusCode};
use wip_protocol::{
    CallOperationRequest, CallOperationResponse, FetchInterfaceRequest, FetchInterfaceResponse,
    INTERFACE_FORMAT_V1, InterfaceDescriptor, InterfaceTarget, ObserveRequest, ObserveResponse,
    ProtocolError, ProtocolInteraction, Target,
};

use crate::response::{self, DecodedResponse};
use crate::{
    BodyKind, ClientResponseError, CodecError, Endpoint, InvalidResponseKind, JSON_CONTENT_TYPE,
    Limits, Route, json,
};

fn request(
    endpoint: &Endpoint,
    route: Route,
    body: Vec<u8>,
) -> Result<Request<Vec<u8>>, http::Error> {
    Request::builder()
        .method(Method::POST)
        .uri(endpoint.route(route).as_str())
        .header(CONTENT_TYPE, JSON_CONTENT_TYPE)
        .header(ACCEPT, "application/json")
        .body(body)
}

fn response(status: StatusCode, body: Vec<u8>) -> Result<Response<Vec<u8>>, http::Error> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, JSON_CONTENT_TYPE)
        .body(body)
}

fn require_response_content_type<B>(response: &Response<B>) -> Result<(), ClientResponseError> {
    if !has_supported_content_coding(response.headers()) {
        return Err(response::unsupported_content_coding(response.status()));
    }
    let valid = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(is_json_content_type);
    if valid {
        Ok(())
    } else if response.status().is_success() {
        Err(ClientResponseError::invalid(
            InvalidResponseKind::InvalidSuccessBody,
            "missing or non-UTF-8 application/json Content-Type",
        ))
    } else {
        Err(response::transport_failure(response.status()))
    }
}

fn is_utf8_charset(value: &str) -> bool {
    let value = value.trim();
    value.eq_ignore_ascii_case("utf-8")
        || value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .is_some_and(|value| value.eq_ignore_ascii_case("utf-8"))
}

/// Returns whether a Content-Type value denotes UTF-8 `application/json`.
///
/// The charset parameter is optional because JSON exchanged by this binding is
/// always UTF-8; when present, its value must be `utf-8`. Other parameters are
/// rejected so intermediaries cannot silently select another representation.
#[must_use]
pub fn is_json_content_type(value: &str) -> bool {
    let mut parts = value.split(';');
    if !parts
        .next()
        .is_some_and(|media| media.trim().eq_ignore_ascii_case("application/json"))
    {
        return false;
    }
    let mut saw_charset = false;
    for parameter in parts {
        let Some((name, value)) = parameter.trim().split_once('=') else {
            return false;
        };
        if !name.trim().eq_ignore_ascii_case("charset") || !is_utf8_charset(value) || saw_charset {
            return false;
        }
        saw_charset = true;
    }
    true
}

fn accepts_json(headers: &HeaderMap) -> bool {
    let mut present = false;
    let mut best_match: Option<((u8, u8), f32)> = None;
    for value in headers.get_all(ACCEPT) {
        present = true;
        let Ok(value) = value.to_str() else {
            continue;
        };
        for range in value.split(',') {
            let mut parts = range.split(';');
            let media_range = parts.next().unwrap_or_default().trim();
            let mut quality = 1.0_f32;
            let mut saw_quality = false;
            let mut saw_charset = false;
            let mut media_parameter_count = 0;
            let mut valid = true;
            for parameter in parts {
                let parameter = parameter.trim();
                let (name, value) = parameter
                    .split_once('=')
                    .map_or((parameter, None), |(name, value)| {
                        (name.trim(), Some(value))
                    });
                if name.is_empty() {
                    valid = false;
                    break;
                }
                if saw_quality {
                    // Parameters after q are Accept extensions and do not
                    // constrain the selected representation.
                    continue;
                }
                if name.eq_ignore_ascii_case("q") {
                    let Some(value) = value else {
                        valid = false;
                        break;
                    };
                    saw_quality = true;
                    match value.trim().parse::<f32>() {
                        Ok(value) if (0.0..=1.0).contains(&value) => quality = value,
                        _ => {
                            valid = false;
                            break;
                        }
                    }
                } else {
                    let Some(value) = value else {
                        valid = false;
                        break;
                    };
                    if !name.eq_ignore_ascii_case("charset")
                        || !is_utf8_charset(value)
                        || saw_charset
                    {
                        valid = false;
                        break;
                    }
                    saw_charset = true;
                    media_parameter_count += 1;
                }
            }
            let media_specificity = if media_range.eq_ignore_ascii_case("application/json") {
                Some(2)
            } else if media_range.eq_ignore_ascii_case("application/*") {
                Some(1)
            } else if media_range == "*/*" {
                Some(0)
            } else {
                None
            };
            if valid && let Some(media_specificity) = media_specificity {
                let specificity = (media_specificity, media_parameter_count);
                match best_match {
                    Some((best_specificity, _)) if best_specificity > specificity => {}
                    Some((best_specificity, best_quality)) if best_specificity == specificity => {
                        best_match = Some((specificity, best_quality.max(quality)));
                    }
                    _ => best_match = Some((specificity, quality)),
                }
            }
        }
    }
    if present {
        best_match.is_some_and(|(_, quality)| quality > 0.0)
    } else {
        true
    }
}

/// A failure while validating inbound HTTP request metadata.
#[derive(Debug)]
pub enum RequestMetadataError {
    /// The request URI does not exactly identify the selected standard route.
    NotFound,
    /// A known standard route was requested with a method other than `POST`.
    MethodNotAllowed,
    /// The request body does not use UTF-8 `application/json` or identity coding.
    UnsupportedMediaType,
    /// The request's `Accept` metadata excludes `application/json`.
    NotAcceptable,
}

impl RequestMetadataError {
    /// Returns the HTTP status for this metadata failure.
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            Self::UnsupportedMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::NotAcceptable => StatusCode::NOT_ACCEPTABLE,
        }
    }

    /// Returns the value for an `Allow` response header, when required.
    #[must_use]
    pub const fn allow(&self) -> Option<&'static str> {
        match self {
            Self::MethodNotAllowed => Some("POST"),
            Self::NotFound | Self::UnsupportedMediaType | Self::NotAcceptable => None,
        }
    }
}

impl Display for RequestMetadataError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("request URI is not a standard WIP route"),
            Self::MethodNotAllowed => formatter.write_str("standard WIP routes require POST"),
            Self::UnsupportedMediaType => {
                formatter.write_str("expected identity-coded UTF-8 application/json Content-Type")
            }
            Self::NotAcceptable => formatter.write_str("request does not accept application/json"),
        }
    }
}

impl Error for RequestMetadataError {}

pub(crate) fn has_supported_content_coding(headers: &HeaderMap) -> bool {
    headers.get_all(CONTENT_ENCODING).iter().all(|value| {
        value.to_str().is_ok_and(|value| {
            let mut codings = value.split(',').map(str::trim);
            let first = codings.next();
            first.is_some_and(|coding| coding.eq_ignore_ascii_case("identity"))
                && codings.all(|coding| coding.eq_ignore_ascii_case("identity"))
        })
    })
}

/// Validates mandatory HTTP metadata for an inbound WIP request.
///
/// Both absolute-form and origin-form request URIs are accepted, but the path
/// must exactly equal the selected endpoint route and a query is forbidden.
/// Unsupported content negotiation is reported as a Transport Binding metadata
/// failure rather than a Core `invalid_request`.
pub fn validate_http_request<B>(
    endpoint: &Endpoint,
    route: Route,
    value: &Request<B>,
) -> Result<(), RequestMetadataError> {
    let expected = endpoint.route(route);
    if value.uri().path() != expected.path() || value.uri().query().is_some() {
        return Err(RequestMetadataError::NotFound);
    }
    if value.method() != Method::POST {
        return Err(RequestMetadataError::MethodNotAllowed);
    }
    let valid_content_type = value
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(is_json_content_type);
    if !valid_content_type || !has_supported_content_coding(value.headers()) {
        return Err(RequestMetadataError::UnsupportedMediaType);
    }
    if !accepts_json(value.headers()) {
        return Err(RequestMetadataError::NotAcceptable);
    }
    Ok(())
}

/// Encodes an `observe` HTTP request.
pub fn encode_observe_request(
    endpoint: &Endpoint,
    value: &ObserveRequest,
    limits: Limits,
) -> Result<Request<Vec<u8>>, EncodeHttpError> {
    value.validate().map_err(CodecError::from)?;
    let body = json::serialize(
        &json::observe_request_to_json(value),
        BodyKind::Request,
        limits,
    )?;
    Ok(request(endpoint, Route::Observe, body)?)
}

/// Decodes an `observe` request body.
pub fn decode_observe_request(body: &[u8], limits: Limits) -> Result<ObserveRequest, CodecError> {
    let value = json::parse(body, BodyKind::Request, limits)?;
    json::observe_request_from_json(&value)
}

/// Encodes a `fetch_interface` HTTP request.
///
/// Validates reference shape without requiring a target Object path.
pub fn encode_fetch_interface_request(
    endpoint: &Endpoint,
    value: &FetchInterfaceRequest,
    limits: Limits,
) -> Result<Request<Vec<u8>>, EncodeHttpError> {
    value.interface.validate().map_err(CodecError::from)?;
    let body = json::serialize(
        &json::fetch_interface_request_to_json(value),
        BodyKind::Request,
        limits,
    )?;
    Ok(request(endpoint, Route::FetchInterface, body)?)
}

/// Decodes a `fetch_interface` request body.
pub fn decode_fetch_interface_request(
    body: &[u8],
    limits: Limits,
) -> Result<FetchInterfaceRequest, CodecError> {
    let value = json::parse(body, BodyKind::Request, limits)?;
    json::fetch_interface_request_from_json(&value)
}

/// Descriptor-independent fields needed to resolve a `call_operation` request.
///
/// A Host decodes this metadata first, fixes the current interface descriptor,
/// then passes the same body and descriptor to [`decode_call_operation_request`].
/// This preserves canonical path/interface/operation lookup and error precedence
/// without treating unvalidated argument JSON as a logical [`wip_protocol::Value`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallOperationMetadata {
    /// Path and optional object-state precondition.
    pub target: Target,
    /// Interface reference and optional interface precondition.
    pub interface: InterfaceTarget,
    /// Operation name used to select a declaration.
    pub operation: String,
}

impl CallOperationMetadata {
    /// Decodes the descriptor-bound request and verifies that routing metadata
    /// still matches the values used to resolve `descriptor`.
    pub fn decode_request(
        &self,
        body: &[u8],
        descriptor: &InterfaceDescriptor,
        limits: Limits,
    ) -> Result<CallOperationRequest, CodecError> {
        let request = decode_call_operation_request(body, descriptor, limits)?;
        if request.target != self.target
            || request.interface != self.interface
            || request.operation != self.operation
        {
            return Err(CodecError::InvalidField {
                field: "request".into(),
                reason: "call metadata changed after descriptor resolution".into(),
            });
        }
        Ok(request)
    }
}

/// Decodes descriptor-independent `call_operation` routing metadata.
///
/// Reference shape is validated, but scope ancestry and `scope_ref` identity
/// are left to Host resolution so `NotFound`, validator failures, and
/// `InterfaceMismatch` retain their protocol precedence.
pub fn decode_call_operation_metadata(
    body: &[u8],
    limits: Limits,
) -> Result<CallOperationMetadata, CodecError> {
    let value = json::parse(body, BodyKind::Request, limits)?;
    let (target, interface, operation) = json::call_request_metadata_from_json(&value)?;
    Ok(CallOperationMetadata {
        target,
        interface,
        operation,
    })
}

/// Encodes a descriptor-bound `call_operation` HTTP request.
pub fn encode_call_operation_request(
    endpoint: &Endpoint,
    value: &CallOperationRequest,
    descriptor: &InterfaceDescriptor,
    limits: Limits,
) -> Result<Request<Vec<u8>>, EncodeHttpError> {
    let value = json::call_request_to_json(value, descriptor)?;
    let body = json::serialize(&value, BodyKind::Request, limits)?;
    Ok(request(endpoint, Route::CallOperation, body)?)
}

/// Decodes a descriptor-bound `call_operation` request body.
pub fn decode_call_operation_request(
    body: &[u8],
    descriptor: &InterfaceDescriptor,
    limits: Limits,
) -> Result<CallOperationRequest, CodecError> {
    let value = json::parse(body, BodyKind::Request, limits)?;
    json::call_request_from_json(&value, descriptor)
}

/// Encodes a successful `observe` response as a direct body with HTTP 200.
pub fn encode_observe_response(
    request_value: &ObserveRequest,
    value: &ObserveResponse,
    limits: Limits,
) -> Result<Response<Vec<u8>>, EncodeHttpError> {
    request_value
        .validate_response(value)
        .map_err(CodecError::from)?;
    let body = json::serialize(
        &json::observe_response_to_json(value),
        BodyKind::Response,
        limits,
    )?;
    Ok(response(StatusCode::OK, body)?)
}

/// Decodes and classifies an `observe` client response.
pub fn decode_observe_response<B: AsRef<[u8]>>(
    request_value: &ObserveRequest,
    value: &Response<B>,
    limits: Limits,
) -> Result<DecodedResponse<ObserveResponse>, ClientResponseError> {
    require_response_content_type(value)?;
    response::decode(
        value.status(),
        value.body().as_ref(),
        ProtocolInteraction::Observe,
        limits,
        |json| {
            let observation = json::observe_response_from_json(json)?;
            request_value.validate_response(&observation)?;
            Ok(observation)
        },
    )
}

/// Encodes a successful `fetch_interface` response as a direct body with HTTP 200.
pub fn encode_fetch_interface_response(
    request_value: &FetchInterfaceRequest,
    value: &FetchInterfaceResponse,
    limits: Limits,
) -> Result<Response<Vec<u8>>, EncodeHttpError> {
    value
        .validate_for(request_value)
        .map_err(CodecError::from)?;
    let body = json::serialize(
        &json::fetch_interface_response_to_json(value),
        BodyKind::Response,
        limits,
    )?;
    Ok(response(StatusCode::OK, body)?)
}

/// Decodes and classifies a `fetch_interface` client response.
pub fn decode_fetch_interface_response<B: AsRef<[u8]>>(
    request_value: &FetchInterfaceRequest,
    value: &Response<B>,
    limits: Limits,
) -> Result<DecodedResponse<FetchInterfaceResponse>, ClientResponseError> {
    require_response_content_type(value)?;
    let decoded = response::decode(
        value.status(),
        value.body().as_ref(),
        ProtocolInteraction::FetchInterface,
        limits,
        |json| {
            let interface = json::fetch_interface_response_from_json(json)?;
            interface.validate_for(request_value)?;
            Ok(interface)
        },
    )?;
    if let DecodedResponse::Success(success) = &decoded
        && success.descriptor.format != INTERFACE_FORMAT_V1
    {
        return Err(ClientResponseError::UnsupportedDescriptorFormat {
            format: success.descriptor.format.clone(),
        });
    }
    Ok(decoded)
}

/// Encodes a successful `call_operation` response as a direct body with HTTP 200.
pub fn encode_call_operation_response(
    request_value: &CallOperationRequest,
    descriptor: &InterfaceDescriptor,
    value: &CallOperationResponse,
    limits: Limits,
) -> Result<Response<Vec<u8>>, EncodeHttpError> {
    let json = json::call_response_to_json(value, descriptor, &request_value.operation)?;
    let body = json::serialize(&json, BodyKind::Response, limits)?;
    Ok(response(StatusCode::OK, body)?)
}

/// Decodes and classifies a descriptor-bound `call_operation` client response.
pub fn decode_call_operation_response<B: AsRef<[u8]>>(
    request_value: &CallOperationRequest,
    descriptor: &InterfaceDescriptor,
    value: &Response<B>,
    limits: Limits,
) -> Result<DecodedResponse<CallOperationResponse>, ClientResponseError> {
    require_response_content_type(value)?;
    response::decode(
        value.status(),
        value.body().as_ref(),
        ProtocolInteraction::CallOperation,
        limits,
        |json| json::call_response_from_json(json, descriptor, &request_value.operation),
    )
}

fn validate_protocol_error(
    interaction: ProtocolInteraction,
    error: &ProtocolError,
) -> Result<(), CodecError> {
    if !error.code.is_allowed_for(interaction) {
        return Err(CodecError::InvalidField {
            field: "error.code".into(),
            reason: format!(
                "code `{}` is not allowed for {interaction:?}",
                response::error_code_name(error.code)
            ),
        });
    }
    Ok(())
}

fn encode_protocol_error_with_status(
    interaction: ProtocolInteraction,
    error: &ProtocolError,
    status: StatusCode,
    limits: Limits,
) -> Result<Response<Vec<u8>>, EncodeHttpError> {
    validate_protocol_error(interaction, error)?;
    let body = json::serialize(&response::error_to_json(error), BodyKind::Response, limits)?;
    Ok(response(status, body)?)
}

/// Encodes a canonical Host error envelope using its standard HTTP status.
///
/// The code must be allowed for `interaction`. This API accepts only
/// [`ProtocolError`], so client-local `InvalidResponse` and
/// `UnsupportedDescriptorFormat` classifications cannot be emitted as Host
/// error envelopes. Logical `resource_limit_exceeded` failures use HTTP 422;
/// representation and deployment limits are not protocol errors.
pub fn encode_protocol_error_response(
    interaction: ProtocolInteraction,
    error: &ProtocolError,
    limits: Limits,
) -> Result<Response<Vec<u8>>, EncodeHttpError> {
    encode_protocol_error_with_status(
        interaction,
        error,
        response::status_for_error(error.code),
        limits,
    )
}

/// Failure while building an outbound framework-neutral HTTP message.
#[derive(Debug)]
pub enum EncodeHttpError {
    /// JSON or protocol encoding failed.
    Codec(CodecError),
    /// The framework-neutral HTTP message builder rejected the URI or headers.
    Http(http::Error),
}

impl std::fmt::Display for EncodeHttpError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Codec(error) => error.fmt(formatter),
            Self::Http(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for EncodeHttpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Codec(error) => Some(error),
            Self::Http(error) => Some(error),
        }
    }
}

impl From<CodecError> for EncodeHttpError {
    fn from(value: CodecError) -> Self {
        Self::Codec(value)
    }
}

impl From<http::Error> for EncodeHttpError {
    fn from(value: http::Error) -> Self {
        Self::Http(value)
    }
}
